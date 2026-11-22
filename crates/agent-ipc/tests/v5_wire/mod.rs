// Frozen from ee9236a03c9775ba2dbfea0dfaa1c04c8f210f19:crates/agent-ipc/src/lib.rs; tests removed.
#![allow(dead_code)]
//! Private, versioned wire contract between the core and the stdio MCP adapter.
//!
//! Compatibility contract (docs/next-release-backlog.md §1.789):
//! - Wire changes are additive: new commands, payloads only for new commands,
//!   and fields. Preserve existing field names, types, and meanings. Defaults
//!   let the core read omitted request fields; old adapters ignore unknown
//!   response fields (response-only additions do not require defaults).
//! - Never add variants to enums an old adapter can receive, including errors
//!   and row kinds, without translating them for its negotiated version.
//! - Preserve old adapters' frame, image, row, and page budgets on their responses.
//! - Adapters deserialize typed IPC and build structuredContent from their own
//!   types, discarding unknown response fields instead of forwarding raw JSON.
//! - Keep the handshake independent of business types: 4-byte LE length, UTF-8
//!   JSON, MAX_FRAME_BYTES cap, request
//!   {"id":…,"command":{"type":"hello","version":N,…}}, response
//!   {"id":…,"result":{"Ok":{"type":"hello","data":{"version":N}}}}.
//! - Freeze shipped adapter types/frames and verify decoding, structuredContent,
//!   and output budgets. Raise MIN only as a planned, documented design change.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

mod read_types;
pub use read_types::*;
mod folder_types;
pub use folder_types::*;

pub const AGENT_PROTOCOL_VERSION: u32 = 5;
pub const MIN_ADAPTER_PROTOCOL_VERSION: u32 = 3;
/// Fixed pipe namespace identifier, unrelated to business IPC versions. Never change it.
pub const AGENT_PIPE_NAMESPACE: &str = "mimageviewer-agent-v3";
pub const VERSION_MISMATCH_MESSAGE: &str = "mIV と AI 連携の版が違います。mIV を終了して起動し直し、AI アプリを再起動 (Claude Code は MCP の再接続) してください。直らない時は、AI アプリを終了してから mIV のファイルを置き換え直してください";
pub const MAX_CLIENT_NAME_CHARS: usize = 128;
pub const MAX_CLIENT_VERSION_CHARS: usize = 64;

/// Self-reported MCP identity; never an authorization credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdentity {
    pub name: String,
    pub version: String,
}
impl ClientIdentity {
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.chars().take(MAX_CLIENT_NAME_CHARS).collect(),
            version: version.chars().take(MAX_CLIENT_VERSION_CHARS).collect(),
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    Tool,
    Internal,
}
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_LIMIT: u32 = 50;
pub const MAX_LIMIT: u32 = 200;
pub const MAX_INFO_LIMIT: u32 = 50;
pub const DEFAULT_FOLDER_LIMIT: u32 = 100;
pub const MAX_THUMBNAIL_ITEMS: usize = 20;
pub const MAX_WRITE_ITEMS: usize = 500;
pub const MAX_WRITE_TAGS: usize = 10;
pub const DEFAULT_THUMBNAIL_SIDE: u32 = 256;
pub const MAX_LEGACY_THUMBNAIL_SIDE: u32 = 512;
pub const MAX_THUMBNAIL_SIDE: u32 = 1568;
/// Absolute decoder cap; negotiated request budgets can be smaller.
pub const MAX_THUMBNAIL_WIRE_IMAGE_BYTES: usize = 1024 * 1024;
pub const MAX_THUMBNAIL_IMAGE_BYTES: usize = 256 * 1024;
pub const MAX_THUMBNAIL_TOTAL_BYTES: usize = 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThumbnailBudget {
    pub max_image_bytes: usize,
    pub max_total_bytes: usize,
    pub max_images: usize,
}

/// Clamp only after the caller has validated the minimum side.
pub fn clamp_thumbnail_side(protocol_version: u32, long_side: u32) -> u32 {
    long_side.min(if protocol_version >= 5 {
        MAX_THUMBNAIL_SIDE
    } else {
        MAX_LEGACY_THUMBNAIL_SIDE
    })
}

/// Shared producer/consumer budget for the negotiated version and request side.
pub fn thumbnail_budget(protocol_version: u32, clamped_long_side: u32) -> ThumbnailBudget {
    if protocol_version >= 5 && clamped_long_side > MAX_LEGACY_THUMBNAIL_SIDE {
        ThumbnailBudget {
            max_image_bytes: MAX_THUMBNAIL_WIRE_IMAGE_BYTES,
            max_total_bytes: 2 * 1024 * 1024,
            max_images: 4,
        }
    } else {
        ThumbnailBudget {
            max_image_bytes: MAX_THUMBNAIL_IMAGE_BYTES,
            max_total_bytes: MAX_THUMBNAIL_TOTAL_BYTES,
            max_images: MAX_THUMBNAIL_ITEMS,
        }
    }
}

/// All non-image budgets count the structured JSON and its escaped text copy.
pub const MAX_ITEM_CHARS: usize = 12_000;
pub const MCP_ENVELOPE_RESERVE_CHARS: usize = 2_000;
pub const MAX_MCP_RESPONSE_CHARS: usize = 40_000;
pub const MAX_PAGE_CHARS: usize = MAX_MCP_RESPONSE_CHARS - MCP_ENVELOPE_RESERVE_CHARS;

/// The exact content contribution shared by core pagination and the MCP envelope.
pub fn mcp_content_chars<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<usize> {
    let serialized = serde_json::to_string(value)?;
    Ok(serialized.chars().count() + serde_json::to_string(&serialized)?.chars().count())
}

/// Digest normalized arguments or an omitted row's identity without retaining
/// long paths or argument arrays inside the cursor/marker.
pub fn wire_digest<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let digest = Sha256::digest(serde_json::to_vec(value)?);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub id: u64,
    pub command: Command,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Hello {
        version: u32,
        client_name: String,
        client_version: String,
        request_kind: RequestKind,
    },
    Status,
    SearchItems {
        query: String,
        kind: Option<ItemKind>,
        limit: Option<u32>,
        cursor: Option<String>,
    },
    ListTags {
        choice: TagChoice,
        limit: Option<u32>,
        cursor: Option<String>,
    },
    ItemsByTag {
        tag: String,
        kind: Option<ItemKind>,
        limit: Option<u32>,
        cursor: Option<String>,
    },
    ListCollections {
        limit: Option<u32>,
        cursor: Option<String>,
    },
    GetCollection {
        id: Option<String>,
        name: Option<String>,
        limit: Option<u32>,
        cursor: Option<String>,
    },
    ListLibrary {
        kind: LibraryKind,
        stars: Option<u8>,
        #[serde(default)]
        max_stars: Option<u8>,
        limit: Option<u32>,
        cursor: Option<String>,
    },
    GetItemInfo {
        items: Vec<ItemRef>,
        limit: Option<u32>,
        cursor: Option<String>,
    },
    GetTranscript {
        item: ItemRef,
        start_secs: Option<f64>,
        cursor: Option<String>,
    },
    ListFolder {
        path: String,
        subresource: Option<String>,
        #[serde(default)]
        min_stars: Option<u8>,
        #[serde(default)]
        max_stars: Option<u8>,
        #[serde(default)]
        kinds: Option<Vec<FolderKind>>,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        max_depth: Option<u32>,
        #[serde(default)]
        name_contains: Option<String>,
        #[serde(default)]
        extensions: Option<Vec<String>>,
        #[serde(default)]
        tagged: Option<bool>,
        limit: Option<u32>,
        cursor: Option<String>,
    },
    SummarizeFolder {
        path: String,
        subresource: Option<String>,
        #[serde(default)]
        min_stars: Option<u8>,
        #[serde(default)]
        max_stars: Option<u8>,
        #[serde(default)]
        kinds: Option<Vec<FolderKind>>,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        max_depth: Option<u32>,
        #[serde(default)]
        name_contains: Option<String>,
        #[serde(default)]
        extensions: Option<Vec<String>>,
        #[serde(default)]
        tagged: Option<bool>,
    },
    GetThumbnail {
        items: Vec<ItemRef>,
        long_side: Option<u32>,
        cursor: Option<String>,
    },
    OpenInViewer {
        path: String,
    },
    CreateCollection {
        name: String,
    },
    RenameCollection {
        id: String,
        name: String,
    },
    AddToCollection {
        id: String,
        items: Vec<ItemRef>,
    },
    RemoveFromCollection {
        id: String,
        items: Vec<ItemRef>,
    },
    AddTags {
        items: Vec<ItemRef>,
        tags: Vec<String>,
    },
    RemoveTags {
        items: Vec<ItemRef>,
        tags: Vec<String>,
    },
    SetRating {
        items: Vec<ItemRef>,
        stars: u8,
    },
    Cancel {
        request_id: u64,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub id: u64,
    pub result: Result<Payload, AgentError>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Payload {
    Hello { version: u32 },
    Status(StatusPayload),
    Items(ItemPage),
    Tags(TagPage),
    Collections(CollectionPage),
    Library(LibraryPage),
    Info(InfoPage),
    Transcript(TranscriptReadResult),
    Folder(FolderPage),
    FolderSummary(FolderSummary),
    Thumbnails(ThumbnailResponse),
    Opened(OpenResult),
    CollectionWrite(CollectionWriteResult),
    TagWrite(MetadataWriteResult),
    RatingWrite(MetadataWriteResult),
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenResult {
    pub status: OpenStatus,
    pub message: String,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenStatus {
    Accepted,
}
/// Completed collection actor result. Item outcomes refer to zero-based input
/// positions, grouped by status so all 500 results fit the MCP output budget.
/// Every input position occurs exactly once; paths are never repeated here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum CollectionWriteResult {
    Created {
        id: String,
    },
    Renamed {
        id: String,
        changed: bool,
    },
    Items {
        id: String,
        outcomes: Vec<CollectionItemOutcome>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CollectionItemOutcome {
    Added {
        indices: Vec<u16>,
    },
    Duplicate {
        indices: Vec<u16>,
    },
    Removed {
        indices: Vec<u16>,
    },
    NotPresent {
        indices: Vec<u16>,
    },
    CapacityRejected {
        indices: Vec<u16>,
    },
    Skipped {
        indices: Vec<u16>,
        reason: CollectionSkipReason,
    },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollectionSkipReason {
    InvalidPath,
    NotFound,
    UnsupportedSubresource,
    UnsupportedType,
}
/// Input positions are grouped by status/reason, with every input represented
/// exactly once. Tag acceptance is Queued; its actual result appears in the
/// in-memory change record. Ratings return Changed/Unchanged after writing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetadataWriteResult {
    pub outcomes: Vec<MetadataItemOutcome>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MetadataItemOutcome {
    Queued {
        indices: Vec<u16>,
    },
    Changed {
        indices: Vec<u16>,
    },
    Unchanged {
        indices: Vec<u16>,
    },
    Skipped {
        indices: Vec<u16>,
        reason: MetadataSkipReason,
    },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MetadataSkipReason {
    InvalidPath,
    NotFound,
    UnsupportedType,
    UnsupportedSubresource,
    Unavailable,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Image,
    Pdf,
    Video,
    Audio,
    Archive,
    Folder,
    Other,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentLevel {
    #[default]
    Disabled,
    ReadOnly,
    ReadAndOpen,
    Organize,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionSnapshot {
    pub level: AgentLevel,
    pub thumbnails: bool,
    pub gps: bool,
    #[serde(default)]
    pub ai_captions: bool,
    #[serde(default)]
    pub transcripts: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TagChoice {
    Pinned,
    Recent,
    Frequent,
    All,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusPayload {
    pub search_scope: Vec<String>,
    pub search_scope_truncated: bool,
    pub app_version: String,
    pub permission: PermissionSnapshot,
    pub fts_ready: bool,
    pub remote_active: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub path: String,
    pub subresource: Option<String>,
    pub name: String,
    pub kind: ItemKind,
    pub tags: Option<Vec<String>>,
    pub container_tags: Option<Vec<String>>,
    pub rating: u8,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ItemPage {
    pub items: Vec<PageRow<Item>>,
    pub cursor: Option<String>,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tag {
    pub name: String,
    pub count: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TagPage {
    pub tags: Vec<PageRow<Tag>>,
    pub cursor: Option<String>,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub count: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollectionPage {
    pub collections: Vec<PageRow<Collection>>,
    pub cursor: Option<String>,
    pub truncated: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Disabled,
    PermissionChanged,
    NotReady,
    BadRequest,
    Conflict,
    VersionMismatch,
    Busy,
    Cancelled,
    Timeout,
    Internal,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentError {
    pub code: ErrorCode,
    pub message: String,
}
impl AgentError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for AgentError {}

/// The adapter accepts only its own version echoed by the core, independently of MCP.
pub fn check_version(version: u32) -> Result<(), AgentError> {
    if version == AGENT_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(version_mismatch())
    }
}

/// Core compatibility is deliberately one-way: older adapters, never newer ones.
pub fn supports_adapter_version(version: u32) -> bool {
    (MIN_ADAPTER_PROTOCOL_VERSION..=AGENT_PROTOCOL_VERSION).contains(&version)
}

pub fn version_mismatch() -> AgentError {
    AgentError::new(ErrorCode::VersionMismatch, VERSION_MISMATCH_MESSAGE)
}

/// A clean EOF returns None; partial headers and bodies are errors. Validate before allocation.
pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> io::Result<Option<T>> {
    let mut header = [0; 4];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    reader.read_exact(&mut header[1..])?;
    let len = u32::from_le_bytes(header) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid agent frame length",
        ));
    }
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "agent frame too large",
        ));
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

/// Absolute lexical resolution shared by core and adapter; never opens the data directory.
pub fn resolve_data_dir(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    Ok(resolved)
}

/// Caller resolves relative paths and dot segments first; this function performs no filesystem I/O.
pub fn derive_pipe_name(resolved_absolute_data_dir: &Path, user_sid: &str) -> io::Result<String> {
    if !resolved_absolute_data_dir.is_absolute()
        || !user_sid.starts_with("S-")
        || !user_sid
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'S' || b == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected absolute data directory and Windows SID",
        ));
    }
    let path = resolved_absolute_data_dir.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "data directory is not Unicode")
    })?;
    let path = path.replace('/', "\\").to_lowercase();
    let path = path
        .strip_prefix(r"\\?\unc\")
        .map(|p| format!(r"\\{p}"))
        .unwrap_or_else(|| path.strip_prefix(r"\\?\").unwrap_or(&path).to_owned());
    let normalized = path.trim_end_matches('\\');
    let digest = Sha256::digest(normalized.as_bytes());
    let hash: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        r"\\.\pipe\{AGENT_PIPE_NAMESPACE}-{user_sid}-{hash}"
    ))
}

#[cfg(windows)]
pub fn current_user_sid() -> io::Result<String> {
    use windows::{
        Win32::{
            Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree},
            Security::{
                Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_QUERY,
                TOKEN_USER, TokenUser,
            },
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        },
        core::PWSTR,
    };
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(io::Error::other)?;
    let result = (|| {
        let mut len = 0;
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut len) };
        if len < std::mem::size_of::<TOKEN_USER>() as u32 {
            return Err(io::Error::last_os_error());
        }
        let mut data = vec![0usize; (len as usize).div_ceil(std::mem::size_of::<usize>())];
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(data.as_mut_ptr().cast()),
                len,
                &mut len,
            )
        }
        .map_err(io::Error::other)?;
        let user = unsafe { &*data.as_ptr().cast::<TOKEN_USER>() };
        let mut text = PWSTR::null();
        unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) }.map_err(io::Error::other)?;
        let result = unsafe { text.to_string() }.map_err(io::Error::other);
        unsafe {
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        }
        result
    })();
    unsafe {
        let _ = CloseHandle(token);
    }
    result
}

pub mod output;
pub mod schema;
pub mod schema_check;
