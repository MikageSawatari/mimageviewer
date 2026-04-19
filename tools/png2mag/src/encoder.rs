//! MAG (MAKI02) image format encoder, 16-color only.
//!
//! Ported from tantanGH/pymag (MIT License) which references the
//! original MAGBIBLE.DOC (Mar.19,1991) by Woody RINN.
//!
//! Input: a paletted image (1 byte per pixel, palette index 0..15) with
//! width that must be a multiple of 4. Output: a complete MAG file.

const SCAN_OFFSET_X: [i32; 16] = [0, -1, -2, -4, 0, -1, 0, -1, -2, 0, -1, -2, 0, -1, -2, 0];
const SCAN_OFFSET_Y: [i32; 16] = [0, 0, 0, 0, -1, -1, -2, -2, -2, -4, -4, -4, -8, -8, -8, -16];
/// Priority order to try reference offsets (skip 0 = fallback to literal).
const SCAN_ORDER: [usize; 15] = [1, 4, 5, 6, 7, 9, 10, 2, 8, 11, 12, 13, 14, 3, 15];

/// Pack 4 palette indices (0..15) starting at byte offset into a u16.
#[inline]
fn pack_pixel(bytes: &[u8], y: usize, x: usize, width: usize) -> u16 {
    let off = y * width + x * 4;
    ((bytes[off] as u16 & 0xf) << 12)
        | ((bytes[off + 1] as u16 & 0xf) << 8)
        | ((bytes[off + 2] as u16 & 0xf) << 4)
        | (bytes[off + 3] as u16 & 0xf)
}

/// Encode a 16-color paletted image into a MAG file.
///
/// `pixels`: width*height bytes, each is a palette index 0..15.
/// `palette`: 16 RGB triplets (48 bytes), each component 0..255 (only the
///            high 4 bits will be stored — MAG uses 4-bit per channel).
///
/// Width must be a multiple of 8 (4 pixels per cell × 2 cells per flag pair).
pub fn encode(
    pixels: &[u8],
    width: usize,
    height: usize,
    palette: &[u8; 48],
    user: &str,
    memo: &str,
) -> Vec<u8> {
    assert!(width % 8 == 0, "MAG width must be a multiple of 8");
    assert_eq!(pixels.len(), width * height);

    let pixels_x = width / 4; // number of 4-pixel cells per row
    let total_cells = height * pixels_x;

    // ── 1. Scan each cell, find best back-reference or fall back to literal.
    let mut raw_flag = vec![0u8; total_cells];
    let mut pixel_buf: Vec<u8> = Vec::new();

    for y in 0..height {
        for x in 0..pixels_x {
            let cur = pack_pixel(pixels, y, x, width);
            let mut best_flag = 0u8;
            for &s in &SCAN_ORDER {
                let sx = x as i32 + SCAN_OFFSET_X[s];
                let sy = y as i32 + SCAN_OFFSET_Y[s];
                if sx < 0 || sy < 0 {
                    continue;
                }
                let scan = pack_pixel(pixels, sy as usize, sx as usize, width);
                if scan == cur {
                    best_flag = s as u8;
                    break;
                }
            }
            raw_flag[y * pixels_x + x] = best_flag;
            if best_flag == 0 {
                pixel_buf.push(((cur & 0xff00) >> 8) as u8);
                pixel_buf.push((cur & 0x00ff) as u8);
            }
        }
    }

    // ── 2. XOR each row with the previous row's flags.
    let mut xor_flag = vec![0u8; total_cells];
    for y in 0..height {
        for x in 0..pixels_x {
            let f0 = raw_flag[y * pixels_x + x];
            let f1 = if y > 0 {
                raw_flag[(y - 1) * pixels_x + x]
            } else {
                0
            };
            xor_flag[y * pixels_x + x] = f0 ^ f1;
        }
    }

    // ── 3. Pack two flags per byte into Flag B; Flag A is a 1-bit-per-pair bitmap.
    assert!(pixels_x % 2 == 0, "pixels_x must be even (width must be multiple of 8)");
    let pairs_per_row = pixels_x / 2;
    let total_pairs = height * pairs_per_row;
    let mut flag_a_bits = vec![0u8; total_pairs];
    let mut flag_b: Vec<u8> = Vec::new();
    for y in 0..height {
        for x in 0..pairs_per_row {
            let f0 = xor_flag[y * pixels_x + x * 2];
            let f1 = xor_flag[y * pixels_x + x * 2 + 1];
            if f0 == 0 && f1 == 0 {
                flag_a_bits[y * pairs_per_row + x] = 0;
            } else {
                flag_a_bits[y * pairs_per_row + x] = 1;
                flag_b.push(((f0 & 0xf) << 4) | (f1 & 0xf));
            }
        }
    }

    // ── 4. Pack Flag A bits into bytes (MSB-first within each byte).
    // Pad bit-count up to a multiple of 8 (decoder reads the trailing bits as 0).
    while flag_a_bits.len() % 8 != 0 {
        flag_a_bits.push(0);
    }
    let mut flag_a_bytes = vec![0u8; flag_a_bits.len() / 8];
    for i in 0..flag_a_bytes.len() {
        let mut b = 0u8;
        for k in 0..8 {
            b |= (flag_a_bits[i * 8 + k] & 1) << (7 - k);
        }
        flag_a_bytes[i] = b;
    }

    // ── 5. Build header sections.
    // 5a. Palette: 16 triplets in GRB order, each component reduced to 4 bits then
    //     replicated to 8 bits (i.e. high nibble = low nibble).
    let mut pal_bytes = [0u8; 48];
    for i in 0..16 {
        let r = palette[i * 3];
        let g = palette[i * 3 + 1];
        let b = palette[i * 3 + 2];
        // MAG palette stores GRB, 4 high bits per component, low nibble = 0.
        pal_bytes[i * 3] = g & 0xf0;
        pal_bytes[i * 3 + 1] = r & 0xf0;
        pal_bytes[i * 3 + 2] = b & 0xf0;
    }

    // 5b. ASCII metadata block: signature + machine code + user + memo + 0x1A.
    let mut meta: Vec<u8> = Vec::new();
    meta.extend_from_slice(b"MAKI02  "); // 8 bytes
    meta.extend_from_slice(b"PYTN "); // 5 bytes machine code (matches pymag)
    let mut user19 = format!("{:<19}", user);
    user19.truncate(19);
    meta.extend_from_slice(user19.as_bytes());
    meta.extend_from_slice(memo.as_bytes());
    meta.push(0x1A);

    // 5c. Header (32 bytes from header start = byte right after 0x1A).
    let header_top = 0x00u8;
    let header_machine_code = 0x00u8;
    let header_machine_flag = 0x00u8;
    let screen_mode = 0x00u8; // 16-color
    let pos_x0 = 0u16;
    let pos_y0 = 0u16;
    let pos_x1 = (pixels_x * 4 - 1) as u16;
    let pos_y1 = (height - 1) as u16;

    let flag_a_offset = (32 + pal_bytes.len()) as u32;
    let flag_b_offset = flag_a_offset + flag_a_bytes.len() as u32;
    let flag_b_size = flag_b.len() as u32;
    let pixel_offset = flag_b_offset + flag_b_size;
    let pixel_size = pixel_buf.len() as u32;

    let mut header: Vec<u8> = Vec::with_capacity(32);
    header.extend_from_slice(&[header_top, header_machine_code, header_machine_flag, screen_mode]);
    header.extend_from_slice(&pos_x0.to_le_bytes());
    header.extend_from_slice(&pos_y0.to_le_bytes());
    header.extend_from_slice(&pos_x1.to_le_bytes());
    header.extend_from_slice(&pos_y1.to_le_bytes());
    header.extend_from_slice(&flag_a_offset.to_le_bytes());
    header.extend_from_slice(&flag_b_offset.to_le_bytes());
    header.extend_from_slice(&flag_b_size.to_le_bytes());
    header.extend_from_slice(&pixel_offset.to_le_bytes());
    header.extend_from_slice(&pixel_size.to_le_bytes());
    debug_assert_eq!(header.len(), 32);

    // ── 6. Concatenate everything.
    let mut out: Vec<u8> = Vec::with_capacity(
        meta.len() + header.len() + pal_bytes.len() + flag_a_bytes.len() + flag_b.len() + pixel_buf.len(),
    );
    out.extend_from_slice(&meta);
    out.extend_from_slice(&header);
    out.extend_from_slice(&pal_bytes);
    out.extend_from_slice(&flag_a_bytes);
    out.extend_from_slice(&flag_b);
    out.extend_from_slice(&pixel_buf);
    out
}
