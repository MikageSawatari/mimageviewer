//! mImageViewer 本体と remote-web 間の IPC プロトコル。
//!
//! GUI や Windows API には依存せず、型・版数・長さ付きフレームだけを共有する。

use std::fmt;
use std::io::{Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Windows ローカル専用の名前付きパイプ名。
// pipe 名は版数から独立させる。版違いも同じ pipe へ到達させ、handshake で
// client / server の両版を観測可能な形で拒否する。
pub const PIPE_NAME: &str = r"\\.\pipe\mimageviewer-remote-thumbnail";
/// 片側だけ変更されたバイナリを接続しないためのプロトコル版数。
pub const PROTOCOL_VERSION: u32 = 2;
pub const MAX_CONTROL_FRAME_BYTES: usize = 128 * 1024;
pub const MAX_RESPONSE_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ClientHello {
    pub protocol_version: u32,
}

impl ClientHello {
    pub fn current() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ServerHello {
    pub protocol_version: u32,
    pub accepted: bool,
}

pub fn negotiate(client_version: u32) -> ServerHello {
    ServerHello {
        protocol_version: PROTOCOL_VERSION,
        accepted: client_version == PROTOCOL_VERSION,
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ThumbnailRequest {
    /// UUID の文字列表現。解釈とお気に入り照合は本体側で行う。
    pub favorite_id: String,
    /// お気に入り root からの相対パス。絶対パスはプロトコルに載せない。
    pub relative_path: String,
    pub target_px: u32,
}

/// 1 本の長寿命接続上で要求と応答を対応付ける識別子。
pub type RequestId = u64;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Thumbnail {
        id: RequestId,
        request: ThumbnailRequest,
    },
}

impl ClientMessage {
    pub fn id(&self) -> RequestId {
        match self {
            Self::Thumbnail { id, .. } => *id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Thumbnail {
        id: RequestId,
        response: ThumbnailResponse,
    },
}

impl ServerMessage {
    pub fn id(&self) -> RequestId {
        match self {
            Self::Thumbnail { id, .. } => *id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum ThumbnailResponse {
    Success { webp_bytes: Vec<u8> },
    Error(ThumbnailError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ThumbnailError {
    pub code: ThumbnailErrorCode,
    pub message: String,
}

impl ThumbnailError {
    pub fn new(code: ThumbnailErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThumbnailErrorCode {
    BadRequest,
    FavoriteNotFound,
    PathRejected,
    NotFound,
    Unsupported,
    GenerationFailed,
    Busy,
    Internal,
}

#[derive(Debug)]
pub enum FrameError {
    Io(std::io::Error),
    TooLarge { length: usize, maximum: usize },
    Encode(String),
    Decode(String),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "IPC I/O error: {error}"),
            Self::TooLarge { length, maximum } => {
                write!(f, "IPC frame is too large ({length} > {maximum})")
            }
            Self::Encode(error) => write!(f, "IPC encode error: {error}"),
            Self::Decode(error) => write!(f, "IPC decode error: {error}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<std::io::Error> for FrameError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<(), FrameError> {
    let bytes = serde_json::to_vec(value).map_err(|error| FrameError::Encode(error.to_string()))?;
    let length = u32::try_from(bytes.len()).map_err(|_| FrameError::TooLarge {
        length: bytes.len(),
        maximum: u32::MAX as usize,
    })?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

pub fn read_frame<R: Read, T: DeserializeOwned>(
    reader: &mut R,
    maximum: usize,
) -> Result<T, FrameError> {
    let mut length_bytes = [0_u8; 4];
    reader.read_exact(&mut length_bytes)?;
    let length = u32::from_le_bytes(length_bytes) as usize;
    if length > maximum {
        return Err(FrameError::TooLarge { length, maximum });
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|error| FrameError::Decode(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_mismatch_is_rejected_and_reports_both_versions() {
        let client_version = PROTOCOL_VERSION + 1;
        let reply = negotiate(client_version);
        assert!(!reply.accepted);
        assert_eq!(reply.protocol_version, PROTOCOL_VERSION);
        assert_ne!(client_version, reply.protocol_version);
    }

    #[test]
    fn current_protocol_version_is_accepted() {
        assert!(negotiate(PROTOCOL_VERSION).accepted);
    }

    #[test]
    fn thumbnail_message_round_trips_through_a_length_delimited_frame() {
        let expected = ClientMessage::Thumbnail {
            id: 42,
            request: ThumbnailRequest {
                favorite_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2".to_owned(),
                relative_path: "album/page.jpg".to_owned(),
                target_px: 384,
            },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        let actual: ClientMessage =
            read_frame(&mut bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn multiplexed_responses_keep_their_request_ids() {
        let first = ServerMessage::Thumbnail {
            id: 9,
            response: ThumbnailResponse::Success {
                webp_bytes: vec![1, 2, 3],
            },
        };
        let second = ServerMessage::Thumbnail {
            id: 4,
            response: ThumbnailResponse::Error(ThumbnailError::new(
                ThumbnailErrorCode::NotFound,
                "missing",
            )),
        };
        let mut first_bytes = Vec::new();
        write_frame(&mut first_bytes, &first).unwrap();
        let mut bytes = first_bytes.clone();
        write_frame(&mut bytes, &second).unwrap();
        let decoded_first: ServerMessage =
            read_frame(&mut bytes.as_slice(), MAX_RESPONSE_FRAME_BYTES).unwrap();
        let decoded_second: ServerMessage = read_frame(
            &mut bytes[first_bytes.len()..].as_ref(),
            MAX_RESPONSE_FRAME_BYTES,
        )
        .unwrap();
        assert_eq!(decoded_first.id(), 9);
        assert_eq!(decoded_second.id(), 4);
        assert_eq!(decoded_first, first);
        assert_eq!(decoded_second, second);
    }

    #[test]
    fn oversized_frame_is_rejected_before_allocating_the_body() {
        let mut bytes = (65_536_u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0; 8]);
        assert!(matches!(
            read_frame::<_, ClientHello>(&mut bytes.as_slice(), 1024),
            Err(FrameError::TooLarge {
                length: 65_536,
                maximum: 1024
            })
        ));
    }
}
