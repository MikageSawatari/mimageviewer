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
pub const PROTOCOL_VERSION: u32 = 13;
pub const MAX_CONTROL_FRAME_BYTES: usize = 128 * 1024;
pub const MAX_RESPONSE_FRAME_BYTES: usize = 64 * 1024 * 1024;

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

/// Web クライアントへ公開できるコンテンツアドレス。
///
/// 実ファイル部分は常に favorite UUID + favorite root からの相対パスで表し、
/// ZIP/PDF 内の位置だけを `subresource` へ追加する。絶対パスをこの型に載せない。
#[derive(Clone, Debug, Deserialize, Hash, PartialEq, Eq, Serialize)]
pub struct RemoteAddress {
    pub favorite_id: String,
    pub relative_path: String,
    pub subresource: RemoteSubresource,
}

impl RemoteAddress {
    pub fn file(favorite_id: impl Into<String>, relative_path: impl Into<String>) -> Self {
        Self {
            favorite_id: favorite_id.into(),
            relative_path: relative_path.into(),
            subresource: RemoteSubresource::File,
        }
    }

    /// トランスポート両端で共通に実行する構文検証。
    /// 実在確認・favorite allowlist・junction 境界は各プロセスが別途検証する。
    pub fn validate_syntax(&self) -> Result<(), AddressError> {
        validate_relative_component_path(&self.relative_path, true)?;
        match &self.subresource {
            RemoteSubresource::File | RemoteSubresource::PdfPage { .. } => Ok(()),
            RemoteSubresource::ZipEntry { entry_name } => validate_zip_path(entry_name, false),
            RemoteSubresource::ZipDirectory { prefix } => validate_zip_path(prefix, true),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Hash, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteSubresource {
    File,
    ZipDirectory {
        prefix: String,
    },
    ZipEntry {
        entry_name: String,
    },
    /// 0-origin のページ番号。
    PdfPage {
        page_number: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressError {
    InvalidRelativePath,
    InvalidZipPath,
}

fn validate_relative_component_path(value: &str, allow_empty: bool) -> Result<(), AddressError> {
    if (!allow_empty && value.is_empty())
        || value.contains('\0')
        || looks_absolute_or_drive_qualified(value)
        || value.split(['/', '\\']).any(|part| part == "..")
    {
        return Err(AddressError::InvalidRelativePath);
    }
    Ok(())
}

fn validate_zip_path(value: &str, directory: bool) -> Result<(), AddressError> {
    if value.contains('\\')
        || validate_relative_component_path(value, directory).is_err()
        || (!directory && value.ends_with('/'))
        || (directory && !value.is_empty() && !value.ends_with('/'))
    {
        return Err(AddressError::InvalidZipPath);
    }
    Ok(())
}

fn looks_absolute_or_drive_qualified(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.starts_with(['/', '\\'])
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ThumbnailRequest {
    pub address: RemoteAddress,
    pub target_px: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ContainerRequest {
    pub address: RemoteAddress,
    /// `None` はコンテナの spread.db 設定、`Some` は Web セッション中だけの上書き。
    pub spread_mode: Option<RemoteSpreadMode>,
    /// Single でも本体の綴じ方向を維持するための session-only 上書き。
    pub reading_direction: Option<RemoteReadingDirection>,
    /// 縦長 viewport 用の表示限定 Single。保存済みモードは変更しない。
    pub force_single_page: bool,
}

/// Web へ公開するページ構成。旧 DB 互換用 `Vertical` は本体側で `Single` へ解決してから返す。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSpreadMode {
    Single,
    Ltr,
    LtrCover,
    Rtl,
    RtlCover,
}

impl RemoteSpreadMode {
    pub fn is_rtl(self) -> bool {
        matches!(self, Self::Rtl | Self::RtlCover)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteReadingDirection {
    Ltr,
    Rtl,
}

impl RemoteReadingDirection {
    pub fn is_rtl(self) -> bool {
        matches!(self, Self::Rtl)
    }
}

/// 本体 UI thread が所有する永続ハンドルで適用する書き込み要求。
///
/// 書き込み種別はこの enum だけへ追加し、IPC / UI 間に種別ごとの pending field を作らない。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteWriteRequest {
    SetSpread {
        address: RemoteAddress,
        spread_mode: RemoteSpreadMode,
        reading_direction: RemoteReadingDirection,
    },
    /// 表示完了済みのページ位置。page fields と record_history は remote-web の
    /// 観測値として受けるが、本体 write worker がローカルと同じ列挙規則で検証・
    /// 正規化してから UI thread へ渡す。
    RecordReadingProgress {
        address: RemoteAddress,
        context_address: RemoteAddress,
        page_index: u32,
        page_number: u32,
        page_count: u32,
        /// write worker がローカルの resume 対象規則から上書きする。
        record_resume: bool,
        record_history: bool,
    },
    SetRating {
        address: RemoteAddress,
        stars: u8,
    },
    SetBookmark {
        address: RemoteAddress,
        context_address: RemoteAddress,
        page_index: u32,
        bookmarked: bool,
    },
    /// App 所有 DB / service からメニューの正本値を再取得する。読み取りだけの
    /// variant も同じ bounded FIFO と UI ownership check を通し、別 queue を持たない。
    GetItemState {
        address: RemoteAddress,
        context_address: RemoteAddress,
        page_index: u32,
        /// write worker がローカルのブックマーク対象条件から上書きする。
        bookmark_supported: bool,
    },
}

impl RemoteWriteRequest {
    pub fn address(&self) -> &RemoteAddress {
        match self {
            Self::SetSpread { address, .. }
            | Self::RecordReadingProgress { address, .. }
            | Self::SetRating { address, .. }
            | Self::SetBookmark { address, .. }
            | Self::GetItemState { address, .. } => address,
        }
    }

    pub fn context_address(&self) -> Option<&RemoteAddress> {
        match self {
            Self::RecordReadingProgress {
                context_address, ..
            }
            | Self::SetBookmark {
                context_address, ..
            }
            | Self::GetItemState {
                context_address, ..
            } => Some(context_address),
            Self::SetSpread { .. } | Self::SetRating { .. } => None,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::SetSpread { .. } => "set_spread",
            Self::RecordReadingProgress { .. } => "record_reading_progress",
            Self::SetRating { .. } => "set_rating",
            Self::SetBookmark { .. } => "set_bookmark",
            Self::GetItemState { .. } => "get_item_state",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum RemoteWriteResponse {
    Success(RemoteWriteResult),
    Error(RemoteWriteError),
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteWriteResult {
    pub item_state: Option<RemoteItemState>,
}

impl RemoteWriteResult {
    pub fn applied() -> Self {
        Self::default()
    }

    pub fn item_state(item_state: RemoteItemState) -> Self {
        Self {
            item_state: Some(item_state),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteItemState {
    pub rating: u8,
    pub bookmark_supported: bool,
    pub bookmarked: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteWriteError {
    pub code: RemoteWriteErrorCode,
    pub message: String,
}

impl RemoteWriteError {
    pub fn new(code: RemoteWriteErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteWriteErrorCode {
    BadRequest,
    FavoriteNotFound,
    PathRejected,
    NotFound,
    Unsupported,
    Busy,
    UiTimeout,
    PersistenceFailed,
    Internal,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerKind {
    Folder,
    Zip,
    Pdf,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerEntryKind {
    Directory,
    Image,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ContainerEntry {
    pub address: RemoteAddress,
    pub name: String,
    pub kind: ContainerEntryKind,
    pub page_count: Option<u32>,
}

/// `pages` は画面上の左→右順。グループ列そのものは本の読み進める順に並ぶ。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PageGroup {
    /// 読み順でこの表示単位の先頭になるページ。履歴 URL とグループ移動の identity。
    pub anchor: RemoteAddress,
    pub pages: Vec<RemoteAddress>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ContainerPayload {
    pub title: String,
    pub kind: ContainerKind,
    /// ZIP の単一ラッパー自動降下後など、実際に表示している位置。
    pub effective_address: RemoteAddress,
    pub entries: Vec<ContainerEntry>,
    /// spread.db または session override から解決したモード。縦持ちでも保持する。
    pub configured_spread_mode: RemoteSpreadMode,
    /// 実際に `page_groups` を構成したモード。縦持ちは `Single`。
    pub effective_spread_mode: RemoteSpreadMode,
    /// Single を含む物理的なページ送り方向。spread.db の reading direction を反映する。
    pub reading_direction: RemoteReadingDirection,
    pub spread_page_gap_px: u32,
    pub page_groups: Vec<PageGroup>,
    pub entry_limit: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum ContainerResponse {
    Success(ContainerPayload),
    Error(MediaError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PageRequest {
    pub address: RemoteAddress,
    /// 表示用ラスタの長辺上限。
    pub target_px: u32,
    pub priority: PagePriority,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PagePriority {
    /// 利用者が現在待っているページ。ローカル UI 用 Critical 枠は使用しない。
    Foreground,
    /// 有界な先読み。foreground より後ろの Normal lane へ入れる。
    Prefetch,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PagePayload {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum PageResponse {
    Success(PagePayload),
    Error(MediaError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct MediaError {
    pub code: MediaErrorCode,
    pub message: String,
}

impl MediaError {
    pub fn new(code: MediaErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaErrorCode {
    BadRequest,
    FavoriteNotFound,
    PathRejected,
    NotFound,
    Unsupported,
    PasswordRequired,
    PageOutOfRange,
    Busy,
    RenderFailed,
    Internal,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct HomeRequest;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SmartFolderSummary {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceKind {
    ReadingHistory,
    Rating,
    Bookshelf,
    Bookmarks,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PlaceSummary {
    pub kind: PlaceKind,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct HomePayload {
    pub smart_folders: Vec<SmartFolderSummary>,
    pub places: Vec<PlaceSummary>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum HomeResponse {
    Success(HomePayload),
    Error(CollectionError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CollectionRequest {
    pub kind: CollectionKind,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CollectionKind {
    ReadingHistory,
    Rating { stars: u8 },
    Bookshelf,
    Bookmarks,
    SmartFolder { definition_id: String },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteEntryKind {
    Folder,
    Image,
    Video,
    Audio,
    Zip,
    Pdf,
    Archive,
    Other,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteEntry {
    pub favorite_id: String,
    /// お気に入り root からの相対パス。絶対パスはこの型に載せない。
    pub relative_path: String,
    pub name: String,
    pub kind: RemoteEntryKind,
    pub detail: Option<String>,
    pub progress_current: Option<u64>,
    pub progress_total: Option<u64>,
    pub rating: Option<u8>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CollectionPayload {
    pub title: String,
    pub thumb_aspect_height_ratio: f64,
    pub entries: Vec<RemoteEntry>,
    /// 応答サイズと初回 thumbnail burst を抑える読み取り専用上限。
    pub entry_limit: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum CollectionResponse {
    Success(CollectionPayload),
    Error(CollectionError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CollectionError {
    pub code: CollectionErrorCode,
    pub message: String,
}

impl CollectionError {
    pub fn new(code: CollectionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionErrorCode {
    BadRequest,
    NotFound,
    Busy,
    Internal,
}

/// 1 本の長寿命接続上で要求と応答を対応付ける識別子。
pub type RequestId = u64;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionConnectionKind {
    Direct,
    Relay,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionPeerInfo {
    pub connection_kind: SessionConnectionKind,
    pub device_name: Option<String>,
}

/// remote-web が確定した公開 URL と、その接続準備状態。
///
/// URL の意味づけは remote-web が所有し、本体はこの値を再検出せず表示だけに使う。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteWebConnectionInfo {
    pub public_url: String,
    pub tailscale_serve: RemoteWebFeatureStatus,
    pub pin_configured: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteWebFeatureStatus {
    Configured,
    NotConfigured,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionAcquireRequest {
    pub client_id: String,
    pub peer: SessionPeerInfo,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionPingRequest {
    pub client_id: String,
    pub user_active: bool,
    pub media_playing: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    NotAcquired,
    LocalInUse,
    Expired,
    Superseded,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionResponse {
    pub status: SessionStatus,
    pub message: String,
}

impl SessionResponse {
    pub fn active() -> Self {
        Self {
            status: SessionStatus::Active,
            message: "remote session active".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    RemoteWebConnectionInfo {
        id: RequestId,
        info: RemoteWebConnectionInfo,
    },
    SessionAcquire {
        id: RequestId,
        request: SessionAcquireRequest,
    },
    SessionPing {
        id: RequestId,
        request: SessionPingRequest,
    },
    SessionActivity {
        id: RequestId,
        client_id: String,
    },
    Thumbnail {
        id: RequestId,
        client_id: String,
        request: ThumbnailRequest,
    },
    Home {
        id: RequestId,
        client_id: String,
        request: HomeRequest,
    },
    Collection {
        id: RequestId,
        client_id: String,
        request: CollectionRequest,
    },
    Container {
        id: RequestId,
        client_id: String,
        request: ContainerRequest,
    },
    Page {
        id: RequestId,
        client_id: String,
        request: PageRequest,
    },
    Write {
        id: RequestId,
        client_id: String,
        request: RemoteWriteRequest,
    },
}

impl ClientMessage {
    pub fn id(&self) -> RequestId {
        match self {
            Self::RemoteWebConnectionInfo { id, .. }
            | Self::SessionAcquire { id, .. }
            | Self::SessionPing { id, .. }
            | Self::SessionActivity { id, .. }
            | Self::Thumbnail { id, .. }
            | Self::Home { id, .. }
            | Self::Collection { id, .. }
            | Self::Container { id, .. }
            | Self::Page { id, .. }
            | Self::Write { id, .. } => *id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    RemoteWebConnectionInfo {
        id: RequestId,
        accepted: bool,
        message: String,
    },
    Session {
        id: RequestId,
        response: SessionResponse,
    },
    Thumbnail {
        id: RequestId,
        response: ThumbnailResponse,
    },
    Home {
        id: RequestId,
        response: HomeResponse,
    },
    Collection {
        id: RequestId,
        response: CollectionResponse,
    },
    Container {
        id: RequestId,
        response: ContainerResponse,
    },
    Page {
        id: RequestId,
        response: PageResponse,
    },
    Write {
        id: RequestId,
        response: RemoteWriteResponse,
    },
}

impl ServerMessage {
    pub fn id(&self) -> RequestId {
        match self {
            Self::RemoteWebConnectionInfo { id, .. }
            | Self::Session { id, .. }
            | Self::Thumbnail { id, .. }
            | Self::Home { id, .. }
            | Self::Collection { id, .. }
            | Self::Container { id, .. }
            | Self::Page { id, .. }
            | Self::Write { id, .. } => *id,
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
    PasswordRequired,
    PageOutOfRange,
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
    fn session_acquire_round_trips_peer_metadata() {
        let expected = ClientMessage::SessionAcquire {
            id: 9,
            request: SessionAcquireRequest {
                client_id: "browser-instance".to_owned(),
                peer: SessionPeerInfo {
                    connection_kind: SessionConnectionKind::Relay,
                    device_name: Some("iphone".to_owned()),
                },
            },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        let actual: ClientMessage =
            read_frame(&mut bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn remote_web_connection_info_round_trips_without_credentials() {
        let expected = ClientMessage::RemoteWebConnectionInfo {
            id: 10,
            info: RemoteWebConnectionInfo {
                public_url: "https://viewer.example.ts.net/".to_owned(),
                tailscale_serve: RemoteWebFeatureStatus::Configured,
                pin_configured: true,
            },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        let actual: ClientMessage =
            read_frame(&mut bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(actual, expected);
        let encoded = String::from_utf8(bytes[4..].to_vec()).unwrap();
        assert!(!encoded.contains("pin="));
        assert!(!encoded.contains("bearer"));
    }

    #[test]
    fn thumbnail_message_round_trips_through_a_length_delimited_frame() {
        let expected = ClientMessage::Thumbnail {
            id: 42,
            client_id: "test-client".to_owned(),
            request: ThumbnailRequest {
                address: RemoteAddress::file(
                    "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2",
                    "album/page.jpg",
                ),
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
    fn page_priority_round_trips_with_the_request() {
        let expected = ClientMessage::Page {
            id: 43,
            client_id: "test-client".to_owned(),
            request: PageRequest {
                address: RemoteAddress {
                    favorite_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2".to_owned(),
                    relative_path: "books/volume.pdf".to_owned(),
                    subresource: RemoteSubresource::PdfPage { page_number: 7 },
                },
                target_px: 1805,
                priority: PagePriority::Prefetch,
            },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        let actual: ClientMessage =
            read_frame(&mut bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn container_spread_request_and_groups_round_trip() {
        let container = RemoteAddress::file("favorite", "books/book.pdf");
        let page = |page_number| RemoteAddress {
            favorite_id: "favorite".to_owned(),
            relative_path: "books/book.pdf".to_owned(),
            subresource: RemoteSubresource::PdfPage { page_number },
        };
        let expected = ServerMessage::Container {
            id: 44,
            response: ContainerResponse::Success(ContainerPayload {
                title: "book.pdf".to_owned(),
                kind: ContainerKind::Pdf,
                effective_address: container,
                entries: Vec::new(),
                configured_spread_mode: RemoteSpreadMode::RtlCover,
                effective_spread_mode: RemoteSpreadMode::RtlCover,
                reading_direction: RemoteReadingDirection::Rtl,
                spread_page_gap_px: 8,
                page_groups: vec![PageGroup {
                    anchor: page(0),
                    pages: vec![page(1), page(0)],
                }],
                entry_limit: 1000,
                truncated: false,
            }),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        let actual: ServerMessage =
            read_frame(&mut bytes.as_slice(), MAX_RESPONSE_FRAME_BYTES).unwrap();
        assert_eq!(actual, expected);

        let request = ContainerRequest {
            address: RemoteAddress::file("favorite", "books/book.pdf"),
            spread_mode: Some(RemoteSpreadMode::Ltr),
            reading_direction: Some(RemoteReadingDirection::Ltr),
            force_single_page: true,
        };
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(encoded.contains("\"spread_mode\":\"ltr\""));
        assert!(encoded.contains("\"reading_direction\":\"ltr\""));
        assert!(encoded.contains("\"force_single_page\":true"));
    }

    #[test]
    fn typed_write_request_round_trips() {
        let expected = ClientMessage::Write {
            id: 45,
            client_id: "test-client".to_owned(),
            request: RemoteWriteRequest::SetSpread {
                address: RemoteAddress::file(
                    "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2",
                    "books/book.pdf",
                ),
                spread_mode: RemoteSpreadMode::RtlCover,
                reading_direction: RemoteReadingDirection::Rtl,
            },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        let actual: ClientMessage =
            read_frame(&mut bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(actual, expected);
        let ClientMessage::Write { request, .. } = actual else {
            unreachable!();
        };
        assert_eq!(request.kind_name(), "set_spread");
    }

    #[test]
    fn every_write_variant_and_item_state_round_trip() {
        let container = RemoteAddress::file("favorite", "books/book.pdf");
        let page = RemoteAddress {
            favorite_id: "favorite".to_owned(),
            relative_path: "books/book.pdf".to_owned(),
            subresource: RemoteSubresource::PdfPage { page_number: 2 },
        };
        let requests = [
            RemoteWriteRequest::RecordReadingProgress {
                address: page.clone(),
                context_address: container.clone(),
                page_index: 2,
                page_number: 3,
                page_count: 12,
                record_resume: true,
                record_history: true,
            },
            RemoteWriteRequest::SetRating {
                address: page.clone(),
                stars: 5,
            },
            RemoteWriteRequest::SetBookmark {
                address: page.clone(),
                context_address: container.clone(),
                page_index: 2,
                bookmarked: true,
            },
            RemoteWriteRequest::GetItemState {
                address: page,
                context_address: container,
                page_index: 2,
                bookmark_supported: true,
            },
        ];
        for request in requests {
            let encoded = serde_json::to_vec(&request).unwrap();
            let decoded: RemoteWriteRequest = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(decoded, request);
        }

        let response =
            RemoteWriteResponse::Success(RemoteWriteResult::item_state(RemoteItemState {
                rating: 4,
                bookmark_supported: true,
                bookmarked: true,
            }));
        let encoded = serde_json::to_vec(&response).unwrap();
        let decoded: RemoteWriteResponse = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, response);
    }

    #[test]
    fn collection_message_keeps_only_favorite_identity_and_relative_path() {
        let expected = ServerMessage::Collection {
            id: 77,
            response: CollectionResponse::Success(CollectionPayload {
                title: "最近読んだ本".to_owned(),
                thumb_aspect_height_ratio: 1.0,
                entry_limit: 1000,
                truncated: false,
                entries: vec![RemoteEntry {
                    favorite_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2".to_owned(),
                    relative_path: "books/volume-1".to_owned(),
                    name: "volume-1".to_owned(),
                    kind: RemoteEntryKind::Folder,
                    detail: Some("3 / 20 ページ".to_owned()),
                    progress_current: Some(3),
                    progress_total: Some(20),
                    rating: None,
                }],
            }),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        let decoded: ServerMessage =
            read_frame(&mut bytes.as_slice(), MAX_RESPONSE_FRAME_BYTES).unwrap();
        assert_eq!(decoded, expected);
        let encoded = String::from_utf8(bytes[4..].to_vec()).unwrap();
        assert!(!encoded.contains(r"C:\\"));
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

    #[test]
    fn zip_entry_address_rejects_traversal_and_windows_aliases() {
        for entry_name in [
            "../secret.jpg",
            "pages/../../secret.jpg",
            r"pages\..\secret.jpg",
            "/absolute.jpg",
            r"C:\secret.jpg",
        ] {
            let address = RemoteAddress {
                favorite_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2".to_owned(),
                relative_path: "books/volume.zip".to_owned(),
                subresource: RemoteSubresource::ZipEntry {
                    entry_name: entry_name.to_owned(),
                },
            };
            assert!(address.validate_syntax().is_err(), "{entry_name:?}");
        }
    }

    #[test]
    fn valid_nested_zip_and_pdf_addresses_round_trip() {
        let addresses = [
            RemoteAddress {
                favorite_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2".to_owned(),
                relative_path: "books/volume.zip".to_owned(),
                subresource: RemoteSubresource::ZipEntry {
                    entry_name: "chapter.zip/pages/001.jpg".to_owned(),
                },
            },
            RemoteAddress {
                favorite_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2".to_owned(),
                relative_path: "books/volume.pdf".to_owned(),
                subresource: RemoteSubresource::PdfPage { page_number: 42 },
            },
        ];
        for address in addresses {
            address.validate_syntax().unwrap();
            let encoded = serde_json::to_vec(&address).unwrap();
            let decoded: RemoteAddress = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(decoded, address);
            assert!(!String::from_utf8(encoded).unwrap().contains("C:"));
        }
    }
}
