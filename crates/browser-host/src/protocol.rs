use crate::{
    ErrorCode,
    positions::{BrowserWindow, View},
};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const NATIVE_HOST_NAME: &str = "to.mikage.mimageviewer";
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const ORIGINS: [&str; 2] = [
    "chrome-extension://fpppoodelfkddekjahhmalmmfdhbpdma/",
    "chrome-extension://dlcbdkekoppppebnhhncghhglkkccdhk/",
];

// Deliberately not Debug: never print a base64 message in an error or diagnostic.
#[derive(Deserialize, Serialize)]
pub struct Request {
    pub v: u32,
    pub kind: String,
    pub data_b64: String,
    pub content_type: String,
    pub page_url: String,
    pub image_url: String,
    pub final_url: String,
    pub view: View,
    pub zoom: f64,
    pub window: BrowserWindow,
}

pub fn validate_length(length: usize) -> Result<(), ErrorCode> {
    if length == 0 || length > MAX_FRAME_BYTES {
        Err(ErrorCode::MessageTooLarge)
    } else {
        Ok(())
    }
}
pub fn read_request(input: &mut impl Read) -> Result<Request, ErrorCode> {
    let mut header = [0; 4];
    input
        .read_exact(&mut header)
        .map_err(|_| ErrorCode::InvalidRequest)?;
    let length = u32::from_le_bytes(header) as usize;
    validate_length(length)?; // Before allocation or reading even one body byte.
    let mut body = vec![0; length];
    input
        .read_exact(&mut body)
        .map_err(|_| ErrorCode::InvalidRequest)?;
    // serde errors can quote input strings; do not forward their text.
    serde_json::from_slice(&body).map_err(|_| ErrorCode::InvalidRequest)
}
pub fn write_response(output: &mut impl Write, response: &crate::Response) -> io::Result<()> {
    let body = serde_json::to_vec(response).map_err(io::Error::other)?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(io::Error::other("native response too large"));
    }
    output.write_all(&(body.len() as u32).to_le_bytes())?;
    output.write_all(&body)?;
    output.flush()
}
