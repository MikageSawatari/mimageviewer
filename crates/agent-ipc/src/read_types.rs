use serde::{Deserialize, Serialize};

use crate::{ItemKind, wire_digest};

/// Normal rows keep their existing JSON shape. An oversized row occupies one
/// position in the page and has a digest identity instead of a truncated path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PageRow<T> {
    Value(T),
    TooLong(TooLongRow),
}

impl<T> From<T> for PageRow<T> {
    fn from(value: T) -> Self {
        Self::Value(value)
    }
}

impl<T> PageRow<T> {
    pub fn too_long(identity: &(impl Serialize + ?Sized)) -> serde_json::Result<Self> {
        Ok(Self::TooLong(TooLongRow {
            id: wire_digest(identity)?,
            error: RowError::TooLong,
        }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TooLongRow {
    pub id: String,
    pub error: RowError,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RowError {
    TooLong,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemRef {
    pub path: String,
    pub subresource: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LibraryKind {
    Favorites,
    ReadingHistory,
    Bookshelf,
    Bookmarks,
    Rated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LibraryItem {
    pub path: String,
    pub subresource: Option<String>,
    pub name: String,
    pub kind: ItemKind,
    pub id: Option<String>,
    pub page: Option<u32>,
    /// UTC Unix milliseconds, matching the owning store's timestamps.
    pub added_at: Option<i64>,
    pub last_read_at: Option<i64>,
    pub rated_at: Option<i64>,
    pub rating: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LibraryPage {
    pub items: Vec<PageRow<LibraryItem>>,
    pub cursor: Option<String>,
    pub truncated: bool,
}

/// Empty values are distinct from a field that does not apply, cannot be read,
/// or requires bytes above the metadata reader's allocation bound.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum InfoField<T> {
    Present(T),
    NotApplicable,
    Unavailable,
    TooLarge,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DimensionUnit {
    Pixels,
    Points,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
    pub unit: DimensionUnit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GpsInfo {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<f64>,
}

/// Only these known EXIF columns may cross the agent boundary. No generic map,
/// UserComment, MakerNote, or raw parser payload is part of this wire type.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ExifInfo {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub captured_at: Option<String>,
    pub exposure_time: Option<String>,
    pub f_number: Option<String>,
    pub iso: Option<String>,
    pub focal_length: Option<String>,
    pub orientation: Option<String>,
    pub gps: Option<GpsInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextValue {
    pub text: String,
    pub truncated: bool,
}

/// General generation fields only. Raw JSON and generator-specific formats are
/// intentionally absent; explanatory values explicitly expose truncation.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationInfo {
    pub tool: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<TextValue>,
    pub negative_prompt: Option<TextValue>,
    pub seed: Option<TextValue>,
    pub steps: Option<TextValue>,
    pub sampler: Option<TextValue>,
    pub cfg: Option<TextValue>,
    pub dimensions: Option<TextValue>,
}

/// Caption access is independent of the other metadata columns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum CaptionField {
    Present(AiCaptionInfo),
    None,
    NotPermitted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiCaptionInfo {
    pub caption: TextValue,
    pub visible_text: TextValue,
    pub keywords: Vec<String>,
    pub model: String,
    /// UTC Unix milliseconds, matching the caption store.
    pub created_at: i64,
    /// Books require cover resolution and intentionally do not report staleness.
    pub stale: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ItemInfo {
    pub item: ItemRef,
    pub name: String,
    pub kind: ItemKind,
    pub size: InfoField<u64>,
    pub modified: InfoField<i64>,
    pub dimensions: InfoField<Dimensions>,
    pub rating: u8,
    pub tags: InfoField<Vec<String>>,
    pub container_size: Option<u64>,
    pub container_modified: Option<i64>,
    pub container_tags: Vec<String>,
    pub exif: InfoField<ExifInfo>,
    pub generation: InfoField<GenerationInfo>,
    pub ai_caption: CaptionField,
    pub container_ai_caption: Option<CaptionField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InfoPage {
    pub items: Vec<PageRow<ItemInfo>>,
    pub cursor: Option<String>,
    pub truncated: bool,
}
