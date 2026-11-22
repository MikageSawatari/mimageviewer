use crate::ErrorCode;
use base64::{Engine, engine::general_purpose::STANDARD};
pub const MAX_IMAGE_BYTES: usize = 48 * 1024 * 1024;

pub fn decode(value: &str) -> Result<Vec<u8>, ErrorCode> {
    if value.is_empty() || value.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
        return Err(ErrorCode::InvalidImage);
    }
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| ErrorCode::InvalidImage)?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ErrorCode::InvalidImage);
    }
    Ok(bytes)
}
pub fn extension(bytes: &[u8]) -> Result<&'static str, ErrorCode> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok("png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok("jpg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok("gif");
    }
    if bytes.starts_with(b"BM") {
        return Ok("bmp");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Ok("webp");
    }
    if bytes.len() >= 16 && &bytes[4..8] == b"ftyp" {
        let size = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        if size >= 16
            && size <= bytes.len()
            && (size - 16).is_multiple_of(4)
            && (matches!(&bytes[8..12], b"avif" | b"avis")
                || bytes[16..size]
                    .chunks_exact(4)
                    .any(|brand| matches!(brand, b"avif" | b"avis")))
        {
            return Ok("avif");
        }
    }
    Err(ErrorCode::InvalidImage)
}
