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
    mimageviewer_capture_file::capture_image_format_from_bytes(bytes)
        .map(|format| format.extension())
        .ok_or(ErrorCode::InvalidImage)
}
