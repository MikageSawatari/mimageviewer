//! Private, versioned wire contract between the core and the stdio MCP adapter.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

mod read_types;
pub use read_types::*;

pub const AGENT_PROTOCOL_VERSION: u32 = 2;
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_LIMIT: u32 = 50;
pub const MAX_LIMIT: u32 = 200;
pub const MAX_INFO_LIMIT: u32 = 50;
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
        limit: Option<u32>,
        cursor: Option<String>,
    },
    GetItemInfo {
        items: Vec<ItemRef>,
        limit: Option<u32>,
        cursor: Option<String>,
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
    Cancelled,
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
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionSnapshot {
    pub level: AgentLevel,
    pub thumbnails: bool,
    pub gps: bool,
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

/// The private core/adapter protocol has exact equality, independent of MCP versions.
pub fn check_version(version: u32) -> Result<(), AgentError> {
    if version == AGENT_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(AgentError::new(
            ErrorCode::VersionMismatch,
            "mIV と AI 連携の版が違います。設定の案内に従って登録し直してください",
        ))
    }
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
        r"\\.\pipe\mimageviewer-agent-v{AGENT_PROTOCOL_VERSION}-{user_sid}-{hash}"
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handshake_version_requires_exact_equality() {
        for version in [0, 1, 2, u32::MAX] {
            if version == AGENT_PROTOCOL_VERSION {
                assert!(check_version(version).is_ok());
            } else {
                let error = check_version(version).unwrap_err();
                assert_eq!(error.code, ErrorCode::VersionMismatch);
                assert_eq!(
                    error.message,
                    "mIV と AI 連携の版が違います。設定の案内に従って登録し直してください"
                );
            }
        }
    }
    #[test]
    fn frame_roundtrip_and_boundaries() {
        let request = Request {
            id: 4,
            command: Command::SearchItems {
                query: "猫".into(),
                kind: Some(ItemKind::Image),
                limit: Some(50),
                cursor: None,
            },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request).unwrap();
        assert_eq!(
            read_frame::<_, Request>(&mut &bytes[..]).unwrap(),
            Some(request)
        );
        assert!(read_frame::<_, Request>(&mut &[][..]).unwrap().is_none());
        for bad in [
            vec![1],
            vec![0, 0, 0, 0],
            ((MAX_FRAME_BYTES + 1) as u32).to_le_bytes().to_vec(),
            vec![2, 0, 0, 0, b'{'],
        ] {
            assert!(read_frame::<_, Request>(&mut &bad[..]).is_err());
        }
        assert!(write_frame(&mut Vec::new(), &"x".repeat(MAX_FRAME_BYTES)).is_err());
        let expanded = "猫".repeat(400_000);
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expanded).unwrap();
        assert!(bytes.len() > 1024 * 1024);
        assert_eq!(
            read_frame::<_, String>(&mut &bytes[..]).unwrap(),
            Some(expanded)
        );
    }
    #[test]
    fn namespace_is_normalized_and_isolated() {
        let sid = "S-1-5-21-123";
        let base = derive_pipe_name(Path::new(r"C:\Users\Test\data"), sid).unwrap();
        assert!(base.starts_with(&format!(
            r"\\.\pipe\mimageviewer-agent-v{AGENT_PROTOCOL_VERSION}-"
        )));
        assert_eq!(
            base,
            derive_pipe_name(Path::new("c:/users/test/data/"), sid).unwrap()
        );
        assert_eq!(
            base,
            derive_pipe_name(Path::new(r"\\?\C:\Users\Test\data"), sid).unwrap()
        );
        assert_ne!(base, derive_pipe_name(Path::new(r"C:\other"), sid).unwrap());
        assert_ne!(
            base,
            derive_pipe_name(Path::new(r"C:\Users\Test\data"), "S-1-5-21-124").unwrap()
        );
        assert!(derive_pipe_name(Path::new("relative"), sid).is_err());
        assert!(derive_pipe_name(Path::new(r"C:\data"), "invalid/name").is_err());
        let unc = derive_pipe_name(Path::new(r"\\server\share\data"), sid).unwrap();
        assert_eq!(
            unc,
            derive_pipe_name(Path::new(r"\\?\unc\SERVER\share\DATA"), sid).unwrap()
        );
    }
    #[test]
    fn lexical_resolution_does_not_require_existing_directories() {
        let resolved = resolve_data_dir(Path::new(
            r"C:\miv-nonexistent-namespace-test\folder\..\data\.\",
        ))
        .unwrap();
        assert_eq!(
            resolved,
            PathBuf::from(r"C:\miv-nonexistent-namespace-test\data")
        );
        assert!(
            resolve_data_dir(Path::new(r".\not-created\..\data"))
                .unwrap()
                .is_absolute()
        );
        assert_eq!(
            derive_pipe_name(&resolved, "S-1-5-21-1").unwrap(),
            derive_pipe_name(
                &resolve_data_dir(Path::new(r"c:/miv-nonexistent-namespace-test/data/")).unwrap(),
                "S-1-5-21-1"
            )
            .unwrap()
        );
    }
    #[test]
    fn typed_wire_variants_roundtrip_and_malformed_json_is_rejected() {
        let commands = vec![
            Command::Hello {
                version: AGENT_PROTOCOL_VERSION,
                client_name: "fixture".into(),
            },
            Command::Status,
            Command::ListTags {
                choice: TagChoice::Pinned,
                limit: Some(17),
                cursor: Some("tags-cursor".into()),
            },
            Command::ItemsByTag {
                tag: "猫".into(),
                kind: None,
                limit: None,
                cursor: Some("opaque".into()),
            },
            Command::ListCollections {
                limit: Some(23),
                cursor: Some("collections-cursor".into()),
            },
            Command::GetCollection {
                id: Some("collection-id".into()),
                name: None,
                limit: Some(1),
                cursor: None,
            },
            Command::ListLibrary {
                kind: LibraryKind::Rated,
                stars: Some(3),
                limit: Some(2),
                cursor: None,
            },
            Command::GetItemInfo {
                items: vec![ItemRef {
                    path: "C:/books/cat.zip".into(),
                    subresource: Some("001.png".into()),
                }],
                limit: Some(MAX_INFO_LIMIT),
                cursor: Some("info-cursor".into()),
            },
            Command::Cancel { request_id: 2 },
        ];
        for command in commands {
            let request = Request { id: 2, command };
            let mut bytes = vec![];
            write_frame(&mut bytes, &request).unwrap();
            assert_eq!(
                read_frame::<_, Request>(&mut &bytes[..]).unwrap(),
                Some(request)
            );
        }
        assert!(read_frame::<_, Request>(&mut &[1, 0, 0, 0, b'{'][..]).is_err());
        let response = Response {
            id: 1,
            result: Err(AgentError::new(
                ErrorCode::PermissionChanged,
                "許可が変更されました",
            )),
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap()["result"]["Err"]["code"],
            "permission_changed"
        );
        let mut bytes = vec![];
        write_frame(&mut bytes, &response).unwrap();
        assert_eq!(
            read_frame::<_, Response>(&mut &bytes[..]).unwrap(),
            Some(response)
        );
    }

    #[test]
    fn permission_order_and_serde_include_read_and_open_without_enabling_defaults() {
        assert!(AgentLevel::Disabled < AgentLevel::ReadOnly);
        assert!(AgentLevel::ReadOnly < AgentLevel::ReadAndOpen);
        assert_eq!(
            serde_json::to_string(&AgentLevel::ReadAndOpen).unwrap(),
            r#""read_and_open""#
        );
        assert_eq!(
            PermissionSnapshot::default(),
            PermissionSnapshot {
                level: AgentLevel::Disabled,
                thumbnails: false,
                gps: false,
            }
        );
    }

    #[test]
    fn content_budget_matches_actual_structured_and_escaped_text_envelope() {
        for text in ["猫🦊", "a\"\\\n\r\t\u{0000}", "", "plain text"] {
            let value = serde_json::json!({"prompt": text.repeat(1000)});
            let structured = serde_json::to_string(&value).unwrap();
            let text = serde_json::to_string(&structured).unwrap();
            assert_eq!(
                mcp_content_chars(&value).unwrap(),
                structured.chars().count() + text.chars().count()
            );
            let empty = serde_json::to_string(&serde_json::json!({
                "structuredContent": null,
                "content": [{"type":"text","text": null}]
            }))
            .unwrap();
            let envelope = serde_json::to_string(&serde_json::json!({
                "structuredContent": value,
                "content": [{"type":"text","text": structured}]
            }))
            .unwrap();
            assert_eq!(
                envelope.chars().count() - (empty.chars().count() - 8),
                mcp_content_chars(&value).unwrap()
            );
        }
        assert_eq!(MAX_ITEM_CHARS, 12_000);
        assert_eq!(
            MAX_PAGE_CHARS + MCP_ENVELOPE_RESERVE_CHARS,
            MAX_MCP_RESPONSE_CHARS
        );
        assert_eq!(MAX_FRAME_BYTES, 4 * 1024 * 1024);
    }

    #[test]
    fn oversized_row_marker_has_stable_bounded_identity_and_preserves_position() {
        let identity = ItemRef {
            path: format!("C:/{}.png", "猫\\\"".repeat(15_000)),
            subresource: Some("001.png".into()),
        };
        let omitted = PageRow::<Item>::too_long(&identity).unwrap();
        let PageRow::TooLong(marker) = &omitted else {
            panic!()
        };
        assert_eq!(marker.id.len(), 64);
        assert_eq!(marker.id, wire_digest(&identity).unwrap());
        assert!(mcp_content_chars(&omitted).unwrap() <= MAX_ITEM_CHARS);
        let page = ItemPage {
            items: vec![omitted],
            cursor: Some("next-input-position".into()),
            truncated: true,
        };
        let json = serde_json::to_value(&page).unwrap();
        assert_eq!(json["items"][0]["error"], "too_long");
        assert_eq!(json["cursor"], "next-input-position");
        assert!(json["items"][0].get("path").is_none());
        assert_eq!(serde_json::from_value::<ItemPage>(json).unwrap(), page);
        let normal = Tag {
            name: "猫".into(),
            count: 1,
        };
        assert_eq!(
            serde_json::to_value(PageRow::Value(normal.clone())).unwrap(),
            serde_json::to_value(normal).unwrap()
        );
    }

    #[test]
    fn typed_metadata_discards_unknown_fields_and_distinguishes_unavailable_from_empty() {
        let exif: ExifInfo = serde_json::from_value(serde_json::json!({
            "make":"camera", "UserComment":"SECRET", "MakerNote":"SECRET", "unknown":"SECRET"
        }))
        .unwrap();
        let json = serde_json::to_string(&exif).unwrap();
        assert!(json.contains("camera"));
        assert!(!json.contains("SECRET"));
        let generation: GenerationInfo = serde_json::from_value(serde_json::json!({
            "prompt":{"text":"cat", "truncated":true},
            "raw":"SECRET", "prompt_json":{"secret":true}, "workflow_json":"SECRET"
        }))
        .unwrap();
        let json = serde_json::to_string(&generation).unwrap();
        assert!(!json.contains("SECRET"));
        assert!(!json.contains("workflow_json"));
        assert_eq!(
            generation.prompt.unwrap(),
            TextValue {
                text: "cat".into(),
                truncated: true
            }
        );
        assert_ne!(
            serde_json::to_value(InfoField::<Vec<String>>::NotApplicable).unwrap(),
            serde_json::to_value(InfoField::Present(Vec::<String>::new())).unwrap()
        );
    }
    #[cfg(windows)]
    #[test]
    fn sid_is_available() {
        assert!(current_user_sid().unwrap().starts_with("S-1-"));
    }
}
