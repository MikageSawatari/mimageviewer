// Frozen from 23d73f026349cadd258f295cd8ecf366194d1f3f:crates/agent-ipc/src/folder_types.rs; tests removed.
use serde::{Deserialize, Serialize};

use super::{Dimensions, ItemRef, PageRow};

/// Counts filtered logical rows before pagination or oversized-row replacement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderSummary {
    pub total: usize,
    pub kinds: std::collections::BTreeMap<FolderKind, usize>,
    pub stars: [usize; 6],
    pub folders: Option<Vec<FolderSummaryFolder>>,
    pub folders_truncated: bool,
    pub book_composition_applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderSummaryFolder {
    pub path: String,
    pub name: String,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderPage {
    pub items: Vec<PageRow<FolderItem>>,
    pub cursor: Option<String>,
    pub truncated: bool,
    pub book_composition_applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderItem {
    pub path: String,
    pub subresource: Option<String>,
    pub name: String,
    pub kind: FolderKind,
    pub size: Option<u64>,
    /// UTC Unix milliseconds, when known.
    pub modified: Option<i64>,
    pub rating: Option<u8>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum FolderKind {
    Folder,
    Image,
    Video,
    Audio,
    Zip,
    Pdf,
    Epub,
    Archive,
    ZipFolder,
    PdfPage,
    ZipEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThumbnailResponse {
    pub page: ThumbnailPage,
    pub images: Vec<ThumbnailImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThumbnailPage {
    pub items: Vec<PageRow<ThumbnailItem>>,
    pub cursor: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThumbnailItem {
    pub item: ItemRef,
    pub status: ThumbnailStatus,
    pub dimensions: Option<Dimensions>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThumbnailStatus {
    Ok,
    TooLarge,
    Unavailable,
    Unsupported,
    HiddenByEdit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThumbnailImage {
    /// Zero-based row in this response page, including oversized-row markers.
    pub item_index: usize,
    /// Base64 on the IPC JSON wire, never a JSON array of bytes.
    #[serde(with = "webp_base64")]
    pub webp: Vec<u8>,
}

mod webp_base64 {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        let max = super::super::MAX_THUMBNAIL_WIRE_IMAGE_BYTES;
        if encoded.len() > max.div_ceil(3) * 4 {
            return Err(D::Error::custom("thumbnail exceeds image byte budget"));
        }
        let bytes = STANDARD.decode(encoded).map_err(D::Error::custom)?;
        if bytes.len() > max {
            return Err(D::Error::custom("thumbnail exceeds image byte budget"));
        }
        Ok(bytes)
    }
}
