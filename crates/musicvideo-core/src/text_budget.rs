//! Coverage preflight for the two comic-core text paint surfaces.
//!
//! Keep the pass list and bitmap dimensions in step with comic-core's
//! `draw_layout_glyphs_into`, `draw_layout_soft_mask`, and `font.rs`. Measuring
//! outline bounds does not rasterize or populate the glyph coverage cache.

use crate::TextStyle;
use comic_core::{GlyphForm, LoadedFont, TextLayout};

pub const MAX_TEXT_PREPARATION_BYTES: usize = 32 * 1024 * 1024;

// The private paint-op/cache representations cannot be named here. This covers
// Vec growth (<2x), Arc headers, hash-table spare buckets and keys, plus the
// retained_bytes' temporary sets. It exceeds their current sizes.
// Per pass/glyph: <2 * 40 B operation Vec capacity, 48 B rotated
// GlyphBitmap + 16 B Arc refcounts, and spare room for temporary pointer sets.
// Tiny Sideways coverage needs this overhead independently of pixel bytes.
const OPERATION_BYTES: usize = 192;
const CACHE_ENTRY_BYTES: usize = 256;
const FIXED_METADATA_BYTES: usize = 256 * 1024;

/// Conservative coverage, paint-program and layout storage, including peak
/// rasterizer/distance-field scratch. Arithmetic/unsupported dimensions fail
/// closed. The caller adds the actual source strings/cache-key storage: markup
/// and font-key lengths cannot be inferred from the shaped layout.
///
/// `FIXED_METADATA_BYTES` also leaves room for the ordinary font outline segment
/// collector. A font's arbitrary outline complexity is not bounded by its bbox;
/// this is a coverage budget, not a limit on parsing adversarial font programs.
pub(super) fn preparation_bound(
    font: &LoadedFont,
    layout: &TextLayout,
    style: &TextStyle,
) -> usize {
    checked_preparation_bound(font, layout, style).unwrap_or(usize::MAX)
}

fn checked_preparation_bound(
    font: &LoadedFont,
    layout: &TextLayout,
    style: &TextStyle,
) -> Option<usize> {
    let passes = passes(style)?;
    let mut call_coverage = 0usize;
    let mut rotated_coverage = 0usize;
    let mut scratch = 0usize;
    let mut cache_entries = 0usize;
    for glyph in &layout.glyphs {
        if !glyph.size.is_finite() || glyph.size < 0.0 {
            return None;
        }
        let Some((_, _, width, height)) = font.glyph_px_bounds_gid(glyph.glyph_id, glyph.size)
        else {
            continue;
        };
        let width = dimension(width)?;
        let height = dimension(height)?;
        if width == 0 || height == 0 {
            continue;
        }
        let base_pixels = area(width, height)?;
        // ab_glyph_rasterizer 0.1.10 Rasterizer::new owns area+4 f32s
        // while font::rasterize_base constructs the separate coverage Vec.
        scratch = scratch.max(base_pixels.checked_add(4)?.checked_mul(4)?);
        for &dilation in &passes {
            let bytes = bitmap_pixels(width, height, dilation)?.checked_mul(4)?;
            // A concurrent comic bake can reset the process-wide generation
            // between ANY two calls. Repeated glyphs, equal dilation widths,
            // echo passes and the two surfaces therefore cannot be assumed to
            // share coverage. Count every occurrence/pass independently.
            call_coverage = call_coverage.checked_add(bytes)?;
            // At most one base and one dilation-map entry per call. Counting
            // both also covers Arc/GlyphBitmap headers if sharing is lost.
            cache_entries = cache_entries.checked_add(2)?;
            if dilation > 0.0 {
                scratch = scratch.max(bytes); // separate distance field
            }
            if glyph.form == GlyphForm::Sideways {
                // rotate_cw always allocates one additional non-cached copy.
                rotated_coverage = rotated_coverage.checked_add(bytes)?;
            }
        }
    }

    // Each call can allocate its output plus its underlying base; a dilated
    // output is at least as large as that base. Twice the per-call output sum
    // bounds both without relying on cache generations. Each surface's calls
    // are already separate: retained_bytes' duplicate Arc accounting introduces
    // no additional allocation beyond this no-sharing case.
    call_coverage
        .checked_mul(2)?
        .checked_add(rotated_coverage.checked_mul(2)?)?
        .checked_add(scratch)?
        .checked_add(
            layout
                .glyphs
                .len()
                .checked_mul(passes.len())?
                .checked_mul(OPERATION_BYTES)?,
        )?
        .checked_add(cache_entries.checked_mul(CACHE_ENTRY_BYTES)?)?
        .checked_add(
            layout
                .glyphs
                .capacity()
                .checked_mul(std::mem::size_of::<comic_core::GlyphPlacement>())?,
        )?
        .checked_add(
            style
                .extra_outlines
                .capacity()
                .checked_mul(std::mem::size_of::<comic_core::StrokeStyle>())?
                .checked_mul(4)?,
        )?
        .checked_add(FIXED_METADATA_BYTES)
}

fn passes(style: &TextStyle) -> Option<Vec<f32>> {
    let mut passes = vec![0.0]; // White glyph surface; transparent body is skipped.
    if let Some(shadow) = style.shadow {
        let spread = finite_nonnegative(shadow.spread_px)?;
        let blur = finite_nonnegative(shadow.blur_px)?.min(48.0);
        passes.push(spread);
        if blur > 0.1 {
            let steps = ((blur / 3.0).ceil() as u32).clamp(2, 8);
            for i in (1..=steps).rev() {
                passes.push(spread + blur * (i as f32 / steps as f32));
            }
        }
    }
    if let Some(echo) = style.echo {
        passes.extend(std::iter::repeat_n(0.0, echo.count.clamp(1, 12) as usize));
    }
    for outline in style.extra_outlines.iter().chain(style.outline.iter()) {
        let width = finite_nonnegative(outline.width_px)?;
        if width > 0.0 {
            passes.push(width);
        }
    }
    Some(passes)
}

fn finite_nonnegative(value: f32) -> Option<f32> {
    value.is_finite().then_some(value.max(0.0))
}

fn dimension(value: f32) -> Option<usize> {
    // comic-core uses signed i32 dimensions and products internally.
    (value.is_finite() && value >= 0.0 && value.ceil() < i32::MAX as f32)
        .then_some(value.ceil() as usize)
}

fn area(width: usize, height: usize) -> Option<usize> {
    let area = width.checked_mul(height)?;
    (area <= i32::MAX as usize).then_some(area)
}

fn bitmap_pixels(width: usize, height: usize, dilation: f32) -> Option<usize> {
    let pad = if dilation > 0.0 {
        dimension(dilation)?.checked_add(1)?.checked_mul(2)?
    } else {
        0
    };
    let width = width.checked_add(pad)?;
    let height = height.checked_add(pad)?;
    if width > i32::MAX as usize || height > i32::MAX as usize {
        return None;
    }
    area(width, height)
}

#[cfg(test)]
#[path = "text_budget_tests.rs"]
mod tests;
