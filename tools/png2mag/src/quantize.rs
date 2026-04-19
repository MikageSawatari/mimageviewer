//! Color quantization to a 16-color palette using NeuQuant.
//!
//! Input: an RGBA8 buffer. Output: (indices, palette[48]).

use color_quant::NeuQuant;

/// Quantize an RGBA image into 16 colors.
/// Returns (palette indices [w*h], 16-color palette as 48 RGB bytes).
pub fn quantize_16(rgba: &[u8], width: u32, height: u32) -> (Vec<u8>, [u8; 48]) {
    // sample_fac=10 is the default speed/quality trade-off in NeuQuant.
    // Smaller (down to 1) = better quality, slower.
    let nq = NeuQuant::new(10, 16, rgba);
    let palette_rgba = nq.color_map_rgba();

    let total = (width * height) as usize;
    let mut indices = Vec::with_capacity(total);
    for px in rgba.chunks_exact(4) {
        indices.push(nq.index_of(px) as u8);
    }

    let mut palette = [0u8; 48];
    for i in 0..16 {
        palette[i * 3] = palette_rgba[i * 4];
        palette[i * 3 + 1] = palette_rgba[i * 4 + 1];
        palette[i * 3 + 2] = palette_rgba[i * 4 + 2];
    }
    (indices, palette)
}
