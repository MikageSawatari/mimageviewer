//! mImageViewer 本体と remote-web 間の IPC プロトコル。
//!
//! GUI や Windows API には依存せず、型・版数・長さ付きフレームだけを共有する。

use std::fmt;
use std::io::{Read, Write};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Windows ローカル専用の名前付きパイプ名。
// pipe 名は版数から独立させる。版違いも同じ pipe へ到達させ、handshake で
// client / server の両版を観測可能な形で拒否する。
pub const PIPE_NAME: &str = r"\\.\pipe\mimageviewer-remote-thumbnail";
/// 片側だけ変更されたバイナリを接続しないためのプロトコル版数。
pub const PROTOCOL_VERSION: u32 = 31;
pub const MAX_CONTROL_FRAME_BYTES: usize = 128 * 1024;
pub const MAX_RESPONSE_FRAME_BYTES: usize = 64 * 1024 * 1024;
/// One wall-clock budget for the complete remote video start path, from core IPC queueing
/// through player/seek/encoder readiness and the first usable playlist.
pub const VIDEO_STREAM_START_BUDGET: Duration = Duration::from_secs(15);
/// A remote AI POST must be admitted by the core within this window. The job itself has no
/// inference deadline after admission.
pub const REMOTE_AI_START_ACCEPT_BUDGET: Duration = Duration::from_secs(2);

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
pub struct FolderListRequest {
    pub address: RemoteAddress,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct FolderListEntry {
    /// Address of the listed cell, relative to its favorite root.
    pub address: RemoteAddress,
    /// Thumbnail source; a video's sidecar image may differ from `address`.
    pub thumbnail_address: RemoteAddress,
    pub name: String,
    pub kind: RemoteEntryKind,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FolderListPayload {
    pub effective_address: RemoteAddress,
    pub thumb_aspect_height_ratio: f64,
    pub entries: Vec<FolderListEntry>,
    /// Time spent scanning the directory and reading metadata.
    pub scan_ms: f64,
    /// Time spent in the canonical core materializer.
    pub materialize_ms: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum FolderListResponse {
    Success(FolderListPayload),
    Error(MediaError),
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteAiAdjustmentValues {
    /// `None` はアップスケールなし、`Some("auto")` は本体の自動選択。
    pub upscale_model: Option<String>,
    /// `None` はデノイズなし。
    pub denoise_model: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteAiModelOption {
    /// `None` は「なし」。モデルキーと表示ラベルは本体を正本にする。
    pub key: Option<String>,
    pub label: String,
    /// 現在の `AiFeatureMode` で新たに選択できるか。保存済みの非許可値は破棄しない。
    pub selectable: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteAiModelCatalog {
    pub upscale: Vec<RemoteAiModelOption>,
    pub denoise: Vec<RemoteAiModelOption>,
}

/// リモートの画像補正パネルが編集できるパラメータ。
///
/// post-filter 等はまだ読み取り専用にし、この型へは入れない。
/// 書き込み側は保存済みの完全な `AdjustParams` にこの差分だけを重ねるため、Web が
/// 非公開フィールドを消したり既定値へ戻したりしない。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RemoteAdjustmentValues {
    pub brightness: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub black_point: u8,
    pub white_point: u8,
    pub midtone: f32,
    pub auto_mode: Option<RemoteAutoMode>,
    pub colorize: RemoteColorizeParams,
    /// 旧 SPA の payload では欠落する。`None` は AI 値を変更しない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai: Option<RemoteAiAdjustmentValues>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAutoMode {
    Auto,
    MangaCleanup,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteColorizeMode {
    #[default]
    Disabled,
    MonochromeOnly,
    AllImages,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteColorizePalette {
    #[default]
    Legacy4Color,
    LegacySkin,
    Custom,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RemoteColorizeControlPoint {
    pub color: [u8; 3],
    pub strength: f32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteToneDensityMethod {
    #[default]
    Off,
    Fast,
    LocalMean,
    Gaussian,
}

/// 本体の `ColorizeParams` と同じ意味・範囲を持つ wire 表現。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RemoteColorizeParams {
    pub mode: RemoteColorizeMode,
    pub mono_tolerance: u8,
    pub palette: RemoteColorizePalette,
    /// Web では編集しないが、Custom の正本値を round-trip で保持する。
    pub control_points: Vec<RemoteColorizeControlPoint>,
    pub luminance_weight: u8,
    pub density_normalization_strength: u8,
    pub tone_method: RemoteToneDensityMethod,
    pub tone_radius: f32,
    pub tone_strength: u8,
}

impl Default for RemoteColorizeParams {
    fn default() -> Self {
        Self {
            mode: RemoteColorizeMode::Disabled,
            mono_tolerance: 12,
            palette: RemoteColorizePalette::Legacy4Color,
            control_points: vec![
                RemoteColorizeControlPoint {
                    color: [0, 0, 0],
                    strength: 3.0,
                },
                RemoteColorizeControlPoint {
                    color: [75, 0, 130],
                    strength: 1.0,
                },
                RemoteColorizeControlPoint {
                    color: [205, 92, 92],
                    strength: 1.0,
                },
                RemoteColorizeControlPoint {
                    color: [245, 222, 179],
                    strength: 1.0,
                },
                RemoteColorizeControlPoint {
                    color: [240, 248, 255],
                    strength: 1.0,
                },
            ],
            luminance_weight: 100,
            density_normalization_strength: 0,
            tone_method: RemoteToneDensityMethod::Off,
            tone_radius: 1.0,
            tone_strength: 100,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAdjustmentScope {
    Standard,
    Page,
}

/// Page 応答だけに適用する未確定値。DB / sidecar / App state は変更しない。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RemoteAdjustmentPreview {
    pub scope: RemoteAdjustmentScope,
    pub values: RemoteAdjustmentValues,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteAdjustmentReadOnlyState {
    pub upscale_label: String,
    pub denoise_label: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RemoteAdjustmentState {
    pub effective_values: RemoteAdjustmentValues,
    pub standard_values: RemoteAdjustmentValues,
    pub selected_scope: RemoteAdjustmentScope,
    pub has_page_override: bool,
    pub standard_label: String,
    pub standard_available: bool,
    /// デスクトップのグローバル保存スロット。Web は読み込みだけを提供する。
    pub colorize_preset_slots: [Option<RemoteColorizeParams>; 4],
    pub ai_model_catalog: RemoteAiModelCatalog,
    /// `AiFeatureMode` を適用した後、effective params が final AI を一つ以上要求するか。
    pub effective_ai_enabled: bool,
    pub read_only: RemoteAdjustmentReadOnlyState,
}

/// 本体 UI thread が所有する永続ハンドルで適用する書き込み要求。
///
/// 書き込み種別はこの enum だけへ追加し、IPC / UI 間に種別ごとの pending field を作らない。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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
    /// 現在の本 / コンテナに属するページブックマークを DB 順のまま取得する。
    /// 一覧の列挙・解決は write worker で行い、UI thread へ I/O を持ち込まない。
    ListBookBookmarks {
        address: RemoteAddress,
        context_address: RemoteAddress,
        page_index: u32,
        /// write worker がローカルのブックマーク対象条件から上書きする。
        bookmark_supported: bool,
    },
    SetBookBookmarkTitle {
        address: RemoteAddress,
        context_address: RemoteAddress,
        page_index: u32,
        id: i64,
        title: String,
    },
    RemoveBookBookmark {
        address: RemoteAddress,
        context_address: RemoteAddress,
        page_index: u32,
        id: i64,
    },
    SetAdjustment {
        address: RemoteAddress,
        scope: RemoteAdjustmentScope,
        values: RemoteAdjustmentValues,
    },
    GetAdjustmentState {
        address: RemoteAddress,
    },
}

impl RemoteWriteRequest {
    pub fn address(&self) -> &RemoteAddress {
        match self {
            Self::SetSpread { address, .. }
            | Self::RecordReadingProgress { address, .. }
            | Self::SetRating { address, .. }
            | Self::SetBookmark { address, .. }
            | Self::GetItemState { address, .. }
            | Self::ListBookBookmarks { address, .. }
            | Self::SetBookBookmarkTitle { address, .. }
            | Self::RemoveBookBookmark { address, .. }
            | Self::SetAdjustment { address, .. }
            | Self::GetAdjustmentState { address } => address,
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
            }
            | Self::ListBookBookmarks {
                context_address, ..
            }
            | Self::SetBookBookmarkTitle {
                context_address, ..
            }
            | Self::RemoveBookBookmark {
                context_address, ..
            } => Some(context_address),
            Self::SetSpread { .. }
            | Self::SetRating { .. }
            | Self::SetAdjustment { .. }
            | Self::GetAdjustmentState { .. } => None,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::SetSpread { .. } => "set_spread",
            Self::RecordReadingProgress { .. } => "record_reading_progress",
            Self::SetRating { .. } => "set_rating",
            Self::SetBookmark { .. } => "set_bookmark",
            Self::GetItemState { .. } => "get_item_state",
            Self::ListBookBookmarks { .. } => "list_book_bookmarks",
            Self::SetBookBookmarkTitle { .. } => "set_book_bookmark_title",
            Self::RemoveBookBookmark { .. } => "remove_book_bookmark",
            Self::SetAdjustment { .. } => "set_adjustment",
            Self::GetAdjustmentState { .. } => "get_adjustment_state",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum RemoteWriteResponse {
    Success(RemoteWriteResult),
    Error(RemoteWriteError),
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RemoteWriteResult {
    pub item_state: Option<RemoteItemState>,
    pub adjustment_state: Option<RemoteAdjustmentState>,
    pub book_bookmarks: Option<RemoteBookBookmarkList>,
}

impl RemoteWriteResult {
    pub fn applied() -> Self {
        Self::default()
    }

    pub fn item_state(item_state: RemoteItemState) -> Self {
        Self {
            item_state: Some(item_state),
            adjustment_state: None,
            book_bookmarks: None,
        }
    }

    pub fn adjustment_state(adjustment_state: RemoteAdjustmentState) -> Self {
        Self {
            item_state: None,
            adjustment_state: Some(adjustment_state),
            book_bookmarks: None,
        }
    }

    pub fn book_bookmarks(book_bookmarks: RemoteBookBookmarkList) -> Self {
        Self {
            item_state: None,
            adjustment_state: None,
            book_bookmarks: Some(book_bookmarks),
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
pub struct RemoteBookBookmarkList {
    pub supported: bool,
    pub rows: Vec<RemoteBookBookmarkRow>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteBookBookmarkRow {
    pub id: i64,
    pub title: Option<String>,
    pub page_index_hint: u32,
    pub page_label: String,
    /// `Some` のときだけ移動・サムネイル取得が可能。解決可否を別 field に重複させない。
    pub target: Option<RemoteBookBookmarkTarget>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteBookBookmarkTarget {
    /// 移動先かつ既存 `/api/thumb` に渡すページ address。
    pub address: RemoteAddress,
    /// 移動先ページを含む、collapse 後の実効コンテナ address。
    pub context_address: RemoteAddress,
    pub item_index: u32,
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

/// ローカルの明示的なコンテナ open と同じ初期遷移。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerOpenMode {
    /// ページ一覧を表示し、保存済み位置があれば選択だけを復元する。
    Grid,
    /// ビューアを先頭ページから開く。
    FirstPage,
    /// ビューアを保存済み位置から開く。無効・未登録なら先頭へフォールバックする。
    ResumePage,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContainerPayload {
    pub title: String,
    pub kind: ContainerKind,
    /// ZIP の単一ラッパー自動降下後など、実際に表示している位置。
    pub effective_address: RemoteAddress,
    pub entries: Vec<ContainerEntry>,
    /// ローカル一覧のサムネイルセルに使う高さ / 幅比。
    pub thumb_aspect_height_ratio: f64,
    /// `book_resume.db` の index を現在の列挙結果で検証し、ページ address に解決した値。
    pub resume_page: Option<RemoteAddress>,
    /// ローカルの auto-fullscreen と `book_open_resume` から解決した初期遷移。
    pub open_mode: ContainerOpenMode,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ContainerResponse {
    Success(ContainerPayload),
    Error(MediaError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PageRequest {
    pub address: RemoteAddress,
    /// 表示用ラスタの長辺上限。
    pub target_px: u32,
    pub priority: PagePriority,
    /// 表示トリムの本キーと、見開き左右 semantics を解決する表示コンテキスト。
    #[serde(default)]
    pub render_context: Option<RemotePageRenderContext>,
    /// 未確定の画像補正プレビュー。永続化せず、この応答の合成だけへ適用する。
    #[serde(default)]
    pub adjustment_preview: Option<RemoteAdjustmentPreview>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemotePageRenderContext {
    pub context_address: RemoteAddress,
    pub display_slot: RemotePageDisplaySlot,
    /// 見開き Auto の上下調停に使う反対側ページ。単ページでは `None`。
    #[serde(default)]
    pub spread_partner: Option<RemoteAddress>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemotePageDisplaySlot {
    #[default]
    Single,
    SpreadLeft,
    SpreadRight,
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
    /// 画素生成側が、解決済み logical path と実際に選んだ subresource から再構成した identity。
    /// HTTP 要求値の echo ではなく、応答画像の取り違え検査に使用する。
    pub identity: RemoteAddress,
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

/// A single acquired remote-control lease.
///
/// `client_id` identifies the device across reconnects. `session_id` is minted by
/// the core for each successful acquisition and must accompany every request that
/// depends on remote-control ownership.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteSessionIdentity {
    pub client_id: String,
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionPingRequest {
    pub owner: RemoteSessionIdentity,
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
    pub session_id: Option<String>,
}

impl SessionResponse {
    pub fn active(session_id: impl Into<String>) -> Self {
        Self {
            status: SessionStatus::Active,
            message: "remote session active".to_owned(),
            session_id: Some(session_id.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoStreamQuality {
    Minimum,
    Low,
    Standard,
    High,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum VideoStreamControlAction {
    Play,
    Pause,
    Volume {
        volume: f64,
    },
    Quality {
        quality: VideoStreamQuality,
        position_secs: f64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoStreamPlaylistKind {
    Master,
    Media,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VideoStreamSegmentIndex {
    Init,
    Media { sequence: u64 },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct VideoStreamSize {
    pub width: u32,
    pub height: u32,
}

/// Browser-side action when a remote video reaches a playback boundary.
///
/// The core resolves this from the same `video_loop_mode` / `video_continuous_mode`
/// settings used by local playback. `Loop` carries every boundary start so a seek performed
/// after the initial stream start can select the same chapter/bookmark interval as the PC.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VideoStreamEndBehavior {
    Stop,
    Loop { boundary_starts_secs: Vec<f64> },
    Next { wrap: bool },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct VideoStreamAudioProcessing {
    pub vst3_requested: bool,
    pub vst3_active: bool,
    pub vst3_active_slots: u32,
    pub vst3_warning: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VideoStreamStartPayload {
    pub session: u64,
    pub generation: u64,
    pub duration_secs: f64,
    pub source_origin_secs: f64,
    pub buffer_target_secs: f64,
    pub encoder: String,
    pub video_size: VideoStreamSize,
    pub codecs: String,
    pub audio_processing: VideoStreamAudioProcessing,
    pub end_behavior: VideoStreamEndBehavior,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct VideoStreamSeekPayload {
    pub generation: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum VideoStreamThumbnailPayload {
    Pending,
    Ready {
        actual_pts_secs: f64,
        width: u32,
        height: u32,
        webp_bytes: Vec<u8>,
    },
    Cleared,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoStreamJumpKind {
    Pin,
    Bookmark,
    Chapter,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VideoStreamJumpEntryId {
    Pin { position_us: i64 },
    Bookmark { bookmark_id: i64 },
    Chapter { start_us: i64 },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VideoStreamJumpEntry {
    pub id: VideoStreamJumpEntryId,
    pub position_secs: f64,
    pub display_time: String,
    pub title: Option<String>,
    pub thumbnail_token: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VideoStreamJumpSection {
    pub kind: VideoStreamJumpKind,
    pub label: String,
    pub entries: Vec<VideoStreamJumpEntry>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VideoStreamJumpListPayload {
    pub sections: Vec<VideoStreamJumpSection>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum VideoStreamJumpThumbnailPayload {
    Found { webp_bytes: Vec<u8> },
    Missing,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct VideoStreamPlaylistPayload {
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum VideoStreamSegmentPayload {
    Found(Vec<u8>),
    NotFound,
    Gone,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VideoStreamStatePayload {
    pub session: u64,
    pub generation: u64,
    pub duration_secs: f64,
    pub source_origin_secs: f64,
    pub generated_start_secs: f64,
    pub generated_end_secs: f64,
    pub ring_start_secs: f64,
    pub ring_end_secs: f64,
    pub ring_earliest_sequence: Option<u64>,
    pub ring_latest_sequence: Option<u64>,
    pub buffer_target_secs: f64,
    pub buffered_secs: f64,
    pub effective_bitrate_bps: u64,
    pub ended: bool,
    pub encoder: String,
    pub video_size: VideoStreamSize,
    pub codecs: String,
    pub audio_processing: VideoStreamAudioProcessing,
    pub play_intent: bool,
    pub volume: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoStreamErrorCode {
    BadRequest,
    FavoriteNotFound,
    PathRejected,
    NotFound,
    Unsupported,
    SessionMismatch,
    GenerationMismatch,
    NotReady,
    Busy,
    UiTimeout,
    StartQueueTimeout,
    StartUiTimeout,
    StartPlayerTimeout,
    StartSeekTimeout,
    StartEncoderTimeout,
    StartPlaylistTimeout,
    ResourceTimeout,
    Failed,
    Internal,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct VideoStreamError {
    pub code: VideoStreamErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteAiPageRequest {
    pub address: RemoteAddress,
    pub target_px: u32,
    /// 通常 Page と同じ表示トリム解決コンテキスト。
    #[serde(default)]
    pub render_context: Option<RemotePageRenderContext>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteAiStartRequest {
    /// Client-generated idempotency key. Repeating it returns the original job.
    pub request_id: String,
    /// Current display group in left-to-right screen order (one or two pages).
    pub pages: Vec<RemoteAiPageRequest>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAiJobState {
    WaitingForLocalDrain,
    PreparingSource,
    LoadingModel,
    Denoising,
    Upscaling,
    Finalizing,
    Cancelling,
    Ready,
    Superseded,
    CancelledByUser,
    DiscardedByHost,
    BackgroundExpired,
    Failed,
}

impl RemoteAiJobState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Ready
                | Self::Superseded
                | Self::CancelledByUser
                | Self::DiscardedByHost
                | Self::BackgroundExpired
                | Self::Failed
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAiProgressPhase {
    WaitingForLocalDrain,
    PreparingSource,
    LoadingModel,
    Denoising,
    Upscaling,
    Finalizing,
    Cancelling,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteAiProgress {
    pub phase: RemoteAiProgressPhase,
    pub page_index: u32,
    pub page_count: u32,
    pub stage_index: u32,
    pub stage_count: u32,
    pub completed_tiles: Option<u32>,
    pub total_tiles: Option<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAiTerminalCode {
    AnimatedGif,
    AnimatedApng,
    AnimatedWebp,
    VectorPdf,
    SizeGate,
    Superseded,
    CancelledByUser,
    DiscardedByHost,
    BackgroundExpired,
    SourceChanged,
    ExecutionFailed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteAiTerminalDetail {
    pub code: RemoteAiTerminalCode,
    pub message: String,
    pub page_index: Option<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAiPageOutcomeState {
    Pending,
    Ready,
    NotApplicable,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteAiPageOutcome {
    pub page_index: u32,
    pub state: RemoteAiPageOutcomeState,
    pub terminal: Option<RemoteAiTerminalDetail>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteAiJobSnapshot {
    pub job_id: String,
    pub request_id: String,
    pub state: RemoteAiJobState,
    pub progress: Option<RemoteAiProgress>,
    pub terminal: Option<RemoteAiTerminalDetail>,
    pub page_count: u32,
    /// Page-local completion. Aggregate `Ready` means every page reached one of these terminal
    /// outcomes; it does not require every page to have replacement bytes.
    pub page_outcomes: Vec<RemoteAiPageOutcome>,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAiJobErrorCode {
    BadRequest,
    StartExpired,
    SessionClosing,
    NotFound,
    Forbidden,
    JobGone,
    NotReady,
    PageNotApplicable,
    PageOutOfRange,
    Internal,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteAiJobError {
    pub code: RemoteAiJobErrorCode,
    pub message: String,
    /// Preserved after heavy terminal metadata/result bytes have expired.
    pub terminal_code: Option<RemoteAiTerminalCode>,
}

impl RemoteAiJobError {
    pub fn new(code: RemoteAiJobErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            terminal_code: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum RemoteAiStartResponse {
    Accepted(RemoteAiJobSnapshot),
    Error(RemoteAiJobError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum RemoteAiStateResponse {
    Success(RemoteAiJobSnapshot),
    Error(RemoteAiJobError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum RemoteAiRecoverableResponse {
    Success(Vec<RemoteAiJobSnapshot>),
    Error(RemoteAiJobError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum RemoteAiCancelResponse {
    Success(RemoteAiJobSnapshot),
    Error(RemoteAiJobError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum RemoteAiResultResponse {
    Success(PagePayload),
    Error(RemoteAiJobError),
}

impl VideoStreamError {
    pub fn new(code: VideoStreamErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum VideoStreamResult<T> {
    Success(T),
    Error(VideoStreamError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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
        owner: RemoteSessionIdentity,
    },
    Thumbnail {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: ThumbnailRequest,
    },
    Home {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: HomeRequest,
    },
    Collection {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: CollectionRequest,
    },
    FolderList {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: FolderListRequest,
    },
    Container {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: ContainerRequest,
    },
    Page {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: PageRequest,
    },
    RemoteAiStart {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: RemoteAiStartRequest,
        accept_before_unix_ms: u64,
    },
    RemoteAiState {
        id: RequestId,
        owner: RemoteSessionIdentity,
        job_id: String,
    },
    RemoteAiRecoverable {
        id: RequestId,
        owner: RemoteSessionIdentity,
    },
    RemoteAiCancel {
        id: RequestId,
        owner: RemoteSessionIdentity,
        job_id: String,
    },
    RemoteAiResult {
        id: RequestId,
        owner: RemoteSessionIdentity,
        job_id: String,
        page_index: u32,
    },
    Write {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: RemoteWriteRequest,
    },
    VideoStreamStart {
        id: RequestId,
        owner: RemoteSessionIdentity,
        address: RemoteAddress,
        quality: VideoStreamQuality,
    },
    VideoStreamControl {
        id: RequestId,
        owner: RemoteSessionIdentity,
        session: u64,
        action: VideoStreamControlAction,
    },
    VideoStreamSeek {
        id: RequestId,
        owner: RemoteSessionIdentity,
        session: u64,
        position_secs: f64,
    },
    VideoStreamThumbnail {
        id: RequestId,
        owner: RemoteSessionIdentity,
        session: u64,
        position_secs: Option<f64>,
    },
    VideoStreamJumpList {
        id: RequestId,
        owner: RemoteSessionIdentity,
        session: u64,
    },
    VideoStreamJumpThumbnail {
        id: RequestId,
        owner: RemoteSessionIdentity,
        session: u64,
        token: String,
    },
    VideoStreamPlaylist {
        id: RequestId,
        owner: RemoteSessionIdentity,
        session: u64,
        generation: u64,
        kind: VideoStreamPlaylistKind,
    },
    VideoStreamSegment {
        id: RequestId,
        owner: RemoteSessionIdentity,
        session: u64,
        generation: u64,
        index: VideoStreamSegmentIndex,
    },
    VideoStreamState {
        id: RequestId,
        owner: RemoteSessionIdentity,
        session: u64,
    },
    VideoStreamStop {
        id: RequestId,
        owner: RemoteSessionIdentity,
        session: u64,
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
            | Self::FolderList { id, .. }
            | Self::Container { id, .. }
            | Self::Page { id, .. }
            | Self::RemoteAiStart { id, .. }
            | Self::RemoteAiState { id, .. }
            | Self::RemoteAiRecoverable { id, .. }
            | Self::RemoteAiCancel { id, .. }
            | Self::RemoteAiResult { id, .. }
            | Self::Write { id, .. }
            | Self::VideoStreamStart { id, .. }
            | Self::VideoStreamControl { id, .. }
            | Self::VideoStreamSeek { id, .. }
            | Self::VideoStreamThumbnail { id, .. }
            | Self::VideoStreamJumpList { id, .. }
            | Self::VideoStreamJumpThumbnail { id, .. }
            | Self::VideoStreamPlaylist { id, .. }
            | Self::VideoStreamSegment { id, .. }
            | Self::VideoStreamState { id, .. }
            | Self::VideoStreamStop { id, .. } => *id,
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
    FolderList {
        id: RequestId,
        response: FolderListResponse,
    },
    Container {
        id: RequestId,
        response: ContainerResponse,
    },
    Page {
        id: RequestId,
        response: PageResponse,
    },
    RemoteAiStart {
        id: RequestId,
        response: RemoteAiStartResponse,
    },
    RemoteAiState {
        id: RequestId,
        response: RemoteAiStateResponse,
    },
    RemoteAiRecoverable {
        id: RequestId,
        response: RemoteAiRecoverableResponse,
    },
    RemoteAiCancel {
        id: RequestId,
        response: RemoteAiCancelResponse,
    },
    RemoteAiResult {
        id: RequestId,
        response: RemoteAiResultResponse,
    },
    Write {
        id: RequestId,
        response: RemoteWriteResponse,
    },
    VideoStreamStart {
        id: RequestId,
        response: VideoStreamResult<VideoStreamStartPayload>,
    },
    VideoStreamControl {
        id: RequestId,
        response: VideoStreamResult<SessionResponse>,
    },
    VideoStreamSeek {
        id: RequestId,
        response: VideoStreamResult<VideoStreamSeekPayload>,
    },
    VideoStreamThumbnail {
        id: RequestId,
        response: VideoStreamResult<VideoStreamThumbnailPayload>,
    },
    VideoStreamJumpList {
        id: RequestId,
        response: VideoStreamResult<VideoStreamJumpListPayload>,
    },
    VideoStreamJumpThumbnail {
        id: RequestId,
        response: VideoStreamResult<VideoStreamJumpThumbnailPayload>,
    },
    VideoStreamPlaylist {
        id: RequestId,
        response: VideoStreamResult<VideoStreamPlaylistPayload>,
    },
    VideoStreamSegment {
        id: RequestId,
        response: VideoStreamResult<VideoStreamSegmentPayload>,
    },
    VideoStreamState {
        id: RequestId,
        response: VideoStreamResult<VideoStreamStatePayload>,
    },
    VideoStreamStop {
        id: RequestId,
        response: VideoStreamResult<()>,
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
            | Self::FolderList { id, .. }
            | Self::Page { id, .. }
            | Self::RemoteAiStart { id, .. }
            | Self::RemoteAiState { id, .. }
            | Self::RemoteAiRecoverable { id, .. }
            | Self::RemoteAiCancel { id, .. }
            | Self::RemoteAiResult { id, .. }
            | Self::Write { id, .. }
            | Self::VideoStreamStart { id, .. }
            | Self::VideoStreamControl { id, .. }
            | Self::VideoStreamSeek { id, .. }
            | Self::VideoStreamThumbnail { id, .. }
            | Self::VideoStreamJumpList { id, .. }
            | Self::VideoStreamJumpThumbnail { id, .. }
            | Self::VideoStreamPlaylist { id, .. }
            | Self::VideoStreamSegment { id, .. }
            | Self::VideoStreamState { id, .. }
            | Self::VideoStreamStop { id, .. } => *id,
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

    fn test_owner(client_id: &str) -> RemoteSessionIdentity {
        RemoteSessionIdentity {
            client_id: client_id.to_owned(),
            session_id: "0123456789abcdef0123456789abcdef".to_owned(),
        }
    }

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
    fn video_stream_state_separates_generation_progress_from_terminal_playhead() {
        let state = VideoStreamStatePayload {
            session: 3,
            generation: 8,
            duration_secs: 600.0,
            source_origin_secs: 240.0,
            generated_start_secs: 240.0,
            generated_end_secs: 300.0,
            ring_start_secs: 240.0,
            ring_end_secs: 300.0,
            ring_earliest_sequence: Some(0),
            ring_latest_sequence: Some(29),
            buffer_target_secs: 60.0,
            buffered_secs: 60.0,
            effective_bitrate_bps: 1_500_000,
            ended: false,
            encoder: "nvenc".to_owned(),
            video_size: VideoStreamSize {
                width: 1920,
                height: 1080,
            },
            codecs: "avc1.640028,mp4a.40.2".to_owned(),
            audio_processing: VideoStreamAudioProcessing {
                vst3_requested: true,
                vst3_active: true,
                vst3_active_slots: 5,
                vst3_warning: None,
            },
            play_intent: true,
            volume: 0.75,
        };

        let value = serde_json::to_value(state).unwrap();
        assert_eq!(value["source_origin_secs"], 240.0);
        assert_eq!(value["generated_end_secs"], 300.0);
        assert_eq!(value["buffer_target_secs"], 60.0);
        assert_eq!(value["play_intent"], true);
        assert_eq!(value["ended"], false);
        assert!(value.get("position_secs").is_none());
        assert!(value.get("playing").is_none());
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
            owner: test_owner("test-client"),
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
            owner: test_owner("test-client"),
            request: PageRequest {
                address: RemoteAddress {
                    favorite_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2".to_owned(),
                    relative_path: "books/volume.pdf".to_owned(),
                    subresource: RemoteSubresource::PdfPage { page_number: 7 },
                },
                target_px: 1805,
                priority: PagePriority::Prefetch,
                render_context: Some(RemotePageRenderContext {
                    context_address: RemoteAddress::file(
                        "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2",
                        "books/volume.pdf",
                    ),
                    display_slot: RemotePageDisplaySlot::SpreadRight,
                    spread_partner: Some(RemoteAddress {
                        favorite_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2".to_owned(),
                        relative_path: "books/volume.pdf".to_owned(),
                        subresource: RemoteSubresource::PdfPage { page_number: 11 },
                    }),
                }),
                adjustment_preview: Some(RemoteAdjustmentPreview {
                    scope: RemoteAdjustmentScope::Page,
                    values: RemoteAdjustmentValues {
                        brightness: 8.0,
                        contrast: 0.0,
                        gamma: 1.0,
                        saturation: 0.0,
                        temperature: 0.0,
                        black_point: 0,
                        white_point: 255,
                        midtone: 1.0,
                        auto_mode: None,
                        colorize: RemoteColorizeParams::default(),
                        ai: None,
                    },
                }),
            },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        let actual: ClientMessage =
            read_frame(&mut bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn folder_list_round_trips_with_sidecar_provenance() {
        let video = RemoteAddress::file("favorite", "movies/sample.mp4");
        let sidecar = RemoteAddress::file("favorite", "movies/sample.jpg");
        let request = ClientMessage::FolderList {
            id: 49,
            owner: test_owner("test-client"),
            request: FolderListRequest {
                address: RemoteAddress::file("favorite", "movies"),
            },
        };
        let response = ServerMessage::FolderList {
            id: 49,
            response: FolderListResponse::Success(FolderListPayload {
                effective_address: RemoteAddress::file("favorite", "movies"),
                thumb_aspect_height_ratio: 9.0 / 16.0,
                entries: vec![FolderListEntry {
                    address: video,
                    thumbnail_address: sidecar,
                    name: "sample.mp4".to_owned(),
                    kind: RemoteEntryKind::Video,
                    size: 123,
                    mtime: 456,
                }],
                scan_ms: 0.2,
                materialize_ms: 0.1,
            }),
        };
        let mut request_bytes = Vec::new();
        write_frame(&mut request_bytes, &request).unwrap();
        let actual_request: ClientMessage =
            read_frame(&mut request_bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(actual_request, request);

        let mut response_bytes = Vec::new();
        write_frame(&mut response_bytes, &response).unwrap();
        let actual_response: ServerMessage =
            read_frame(&mut response_bytes.as_slice(), MAX_RESPONSE_FRAME_BYTES).unwrap();
        assert_eq!(actual_response, response);
        let encoded = String::from_utf8(response_bytes[4..].to_vec()).unwrap();
        assert!(!encoded.contains(":\\"));
    }

    #[test]
    fn protocol_v31_page_identity_session_epoch_auto_trim_partner_video_and_audio_status_round_trip()
     {
        assert_eq!(PROTOCOL_VERSION, 31);
        let requests = [
            ClientMessage::VideoStreamStart {
                id: 50,
                owner: test_owner("test-client"),
                address: RemoteAddress::file(
                    "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2",
                    "movies/sample.mp4",
                ),
                quality: VideoStreamQuality::Standard,
            },
            ClientMessage::VideoStreamPlaylist {
                id: 51,
                owner: test_owner("test-client"),
                session: 7,
                generation: 9,
                kind: VideoStreamPlaylistKind::Media,
            },
            ClientMessage::VideoStreamSegment {
                id: 52,
                owner: test_owner("test-client"),
                session: 7,
                generation: 9,
                index: VideoStreamSegmentIndex::Media { sequence: 12 },
            },
            ClientMessage::VideoStreamThumbnail {
                id: 53,
                owner: test_owner("test-client"),
                session: 7,
                position_secs: Some(42.5),
            },
            ClientMessage::VideoStreamJumpList {
                id: 54,
                owner: test_owner("test-client"),
                session: 7,
            },
            ClientMessage::VideoStreamJumpThumbnail {
                id: 55,
                owner: test_owner("test-client"),
                session: 7,
                token: "v1:bookmark:3:abc".to_owned(),
            },
        ];
        for expected in requests {
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &expected).unwrap();
            let actual: ClientMessage =
                read_frame(&mut bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
            assert_eq!(actual, expected);
        }

        for expected in [
            ServerMessage::VideoStreamStart {
                id: 50,
                response: VideoStreamResult::Success(VideoStreamStartPayload {
                    session: 7,
                    generation: 9,
                    duration_secs: 284.5,
                    source_origin_secs: 0.0,
                    buffer_target_secs: 60.0,
                    encoder: "software".to_owned(),
                    video_size: VideoStreamSize {
                        width: 1920,
                        height: 1080,
                    },
                    codecs: "avc1.640028,mp4a.40.2".to_owned(),
                    audio_processing: VideoStreamAudioProcessing {
                        vst3_requested: true,
                        vst3_active: false,
                        vst3_active_slots: 0,
                        vst3_warning: Some("VST3 を適用できませんでした".to_owned()),
                    },
                    end_behavior: VideoStreamEndBehavior::Next { wrap: true },
                }),
            },
            ServerMessage::VideoStreamSegment {
                id: 52,
                response: VideoStreamResult::Success(VideoStreamSegmentPayload::Gone),
            },
            ServerMessage::VideoStreamPlaylist {
                id: 51,
                response: VideoStreamResult::Error(VideoStreamError::new(
                    VideoStreamErrorCode::StartEncoderTimeout,
                    "encoder deadline",
                )),
            },
            ServerMessage::VideoStreamThumbnail {
                id: 53,
                response: VideoStreamResult::Success(VideoStreamThumbnailPayload::Ready {
                    actual_pts_secs: 42.466,
                    width: 320,
                    height: 180,
                    webp_bytes: vec![1, 2, 3],
                }),
            },
            ServerMessage::VideoStreamJumpList {
                id: 54,
                response: VideoStreamResult::Success(VideoStreamJumpListPayload {
                    sections: vec![VideoStreamJumpSection {
                        kind: VideoStreamJumpKind::Bookmark,
                        label: "ブックマーク".to_owned(),
                        entries: vec![VideoStreamJumpEntry {
                            id: VideoStreamJumpEntryId::Bookmark { bookmark_id: 3 },
                            position_secs: 42.5,
                            display_time: "0:42.500".to_owned(),
                            title: Some("見どころ".to_owned()),
                            thumbnail_token: Some("v1:bookmark:3:abc".to_owned()),
                        }],
                    }],
                }),
            },
            ServerMessage::VideoStreamJumpThumbnail {
                id: 55,
                response: VideoStreamResult::Success(VideoStreamJumpThumbnailPayload::Found {
                    webp_bytes: vec![4, 5, 6],
                }),
            },
        ] {
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &expected).unwrap();
            let actual: ServerMessage =
                read_frame(&mut bytes.as_slice(), MAX_RESPONSE_FRAME_BYTES).unwrap();
            assert_eq!(actual, expected);
        }
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
                thumb_aspect_height_ratio: 1.5,
                resume_page: Some(page(1)),
                open_mode: ContainerOpenMode::ResumePage,
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
            owner: test_owner("test-client"),
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
                address: page.clone(),
                context_address: container.clone(),
                page_index: 2,
                bookmark_supported: true,
            },
            RemoteWriteRequest::ListBookBookmarks {
                address: page.clone(),
                context_address: container.clone(),
                page_index: 2,
                bookmark_supported: true,
            },
            RemoteWriteRequest::SetBookBookmarkTitle {
                address: page.clone(),
                context_address: container.clone(),
                page_index: 2,
                id: 41,
                title: "見開き".to_owned(),
            },
            RemoteWriteRequest::RemoveBookBookmark {
                address: page.clone(),
                context_address: container,
                page_index: 2,
                id: 41,
            },
            RemoteWriteRequest::SetAdjustment {
                address: page.clone(),
                scope: RemoteAdjustmentScope::Page,
                values: RemoteAdjustmentValues {
                    brightness: 12.0,
                    contrast: -4.0,
                    gamma: 1.1,
                    saturation: 5.0,
                    temperature: 2.0,
                    black_point: 3,
                    white_point: 250,
                    midtone: 0.9,
                    auto_mode: Some(RemoteAutoMode::Auto),
                    colorize: RemoteColorizeParams {
                        mode: RemoteColorizeMode::AllImages,
                        palette: RemoteColorizePalette::Custom,
                        control_points: vec![
                            RemoteColorizeControlPoint {
                                color: [4, 8, 18],
                                strength: 2.0,
                            },
                            RemoteColorizeControlPoint {
                                color: [245, 235, 210],
                                strength: 0.75,
                            },
                        ],
                        ..RemoteColorizeParams::default()
                    },
                    ai: Some(RemoteAiAdjustmentValues {
                        upscale_model: Some("auto".to_owned()),
                        denoise_model: None,
                    }),
                },
            },
            RemoteWriteRequest::GetAdjustmentState { address: page },
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

        let bookmark_list = RemoteWriteResponse::Success(RemoteWriteResult::book_bookmarks(
            RemoteBookBookmarkList {
                supported: true,
                rows: vec![RemoteBookBookmarkRow {
                    id: 41,
                    title: Some("見開き".to_owned()),
                    page_index_hint: 8,
                    page_label: "009.jpg".to_owned(),
                    target: Some(RemoteBookBookmarkTarget {
                        address: RemoteAddress {
                            favorite_id: "favorite".to_owned(),
                            relative_path: "books/book.zip".to_owned(),
                            subresource: RemoteSubresource::ZipEntry {
                                entry_name: "chapter/009.jpg".to_owned(),
                            },
                        },
                        context_address: RemoteAddress {
                            favorite_id: "favorite".to_owned(),
                            relative_path: "books/book.zip".to_owned(),
                            subresource: RemoteSubresource::ZipDirectory {
                                prefix: "chapter/".to_owned(),
                            },
                        },
                        item_index: 8,
                    }),
                }],
            },
        ));
        let encoded = serde_json::to_vec(&bookmark_list).unwrap();
        let decoded: RemoteWriteResponse = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, bookmark_list);

        let adjustment = RemoteWriteResponse::Success(RemoteWriteResult::adjustment_state(
            RemoteAdjustmentState {
                effective_values: RemoteAdjustmentValues {
                    brightness: 1.0,
                    contrast: 2.0,
                    gamma: 1.0,
                    saturation: 0.0,
                    temperature: 0.0,
                    black_point: 0,
                    white_point: 255,
                    midtone: 1.0,
                    auto_mode: None,
                    colorize: RemoteColorizeParams::default(),
                    ai: Some(RemoteAiAdjustmentValues {
                        upscale_model: Some("auto".to_owned()),
                        denoise_model: None,
                    }),
                },
                standard_values: RemoteAdjustmentValues {
                    brightness: 0.0,
                    contrast: 0.0,
                    gamma: 1.0,
                    saturation: 0.0,
                    temperature: 0.0,
                    black_point: 0,
                    white_point: 255,
                    midtone: 1.0,
                    auto_mode: None,
                    colorize: RemoteColorizeParams::default(),
                    ai: Some(RemoteAiAdjustmentValues {
                        upscale_model: None,
                        denoise_model: None,
                    }),
                },
                selected_scope: RemoteAdjustmentScope::Page,
                has_page_override: true,
                standard_label: "標準（共通）".to_owned(),
                standard_available: true,
                colorize_preset_slots: [
                    Some(RemoteColorizeParams {
                        mode: RemoteColorizeMode::MonochromeOnly,
                        ..RemoteColorizeParams::default()
                    }),
                    None,
                    None,
                    None,
                ],
                ai_model_catalog: RemoteAiModelCatalog {
                    upscale: vec![
                        RemoteAiModelOption {
                            key: None,
                            label: "なし".to_owned(),
                            selectable: true,
                        },
                        RemoteAiModelOption {
                            key: Some("auto".to_owned()),
                            label: "自動 (画像タイプ判別)".to_owned(),
                            selectable: true,
                        },
                    ],
                    denoise: vec![RemoteAiModelOption {
                        key: None,
                        label: "なし".to_owned(),
                        selectable: true,
                    }],
                },
                effective_ai_enabled: true,
                read_only: RemoteAdjustmentReadOnlyState {
                    upscale_label: "なし".to_owned(),
                    denoise_label: "なし".to_owned(),
                },
            },
        ));
        let encoded = serde_json::to_vec(&adjustment).unwrap();
        let decoded: RemoteWriteResponse = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, adjustment);
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

    #[test]
    fn page_payload_carries_the_rendered_identity_across_ipc() {
        let identity = RemoteAddress {
            favorite_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2".to_owned(),
            relative_path: "books/volume.pdf".to_owned(),
            subresource: RemoteSubresource::PdfPage { page_number: 1 },
        };
        let expected = ServerMessage::Page {
            id: 88,
            response: PageResponse::Success(PagePayload {
                bytes: vec![1, 2, 3],
                content_type: "image/jpeg".to_owned(),
                width: 1200,
                height: 1800,
                identity: identity.clone(),
            }),
        };
        let mut frame = Vec::new();
        write_frame(&mut frame, &expected).unwrap();
        let decoded: ServerMessage =
            read_frame(&mut frame.as_slice(), MAX_RESPONSE_FRAME_BYTES).unwrap();
        assert_eq!(decoded, expected);
        let ServerMessage::Page {
            response: PageResponse::Success(payload),
            ..
        } = decoded
        else {
            unreachable!();
        };
        assert_eq!(payload.identity, identity);
    }

    #[test]
    fn protocol_v25_remote_ai_messages_and_page_outcomes_round_trip() {
        let address = RemoteAddress::file("30d6c167-7148-4f3e-9a5a-21c5fd31ecb2", "pages/001.png");
        let start = RemoteAiStartRequest {
            request_id: "request-1".to_owned(),
            pages: vec![RemoteAiPageRequest {
                address,
                target_px: 1600,
                render_context: None,
            }],
        };
        let client_messages = vec![
            ClientMessage::RemoteAiStart {
                id: 1,
                owner: test_owner("phone"),
                request: start,
                accept_before_unix_ms: 1_900_000_000_000,
            },
            ClientMessage::RemoteAiState {
                id: 2,
                owner: test_owner("phone"),
                job_id: "7-1".to_owned(),
            },
            ClientMessage::RemoteAiRecoverable {
                id: 3,
                owner: test_owner("phone"),
            },
            ClientMessage::RemoteAiCancel {
                id: 4,
                owner: test_owner("phone"),
                job_id: "7-1".to_owned(),
            },
            ClientMessage::RemoteAiResult {
                id: 5,
                owner: test_owner("phone"),
                job_id: "7-1".to_owned(),
                page_index: 0,
            },
        ];
        for message in client_messages {
            let mut frame = Vec::new();
            write_frame(&mut frame, &message).unwrap();
            let decoded: ClientMessage =
                read_frame(&mut frame.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
            assert_eq!(decoded, message);
        }

        let snapshot = RemoteAiJobSnapshot {
            job_id: "7-1".to_owned(),
            request_id: "request-1".to_owned(),
            state: RemoteAiJobState::Ready,
            progress: None,
            terminal: None,
            page_count: 1,
            page_outcomes: vec![RemoteAiPageOutcome {
                page_index: 0,
                state: RemoteAiPageOutcomeState::NotApplicable,
                terminal: Some(RemoteAiTerminalDetail {
                    code: RemoteAiTerminalCode::VectorPdf,
                    message: "not applicable".to_owned(),
                    page_index: Some(0),
                }),
            }],
            created_unix_ms: 10,
            updated_unix_ms: 20,
        };
        let message = ServerMessage::RemoteAiState {
            id: 2,
            response: RemoteAiStateResponse::Success(snapshot),
        };
        let mut frame = Vec::new();
        write_frame(&mut frame, &message).unwrap();
        let decoded: ServerMessage =
            read_frame(&mut frame.as_slice(), MAX_RESPONSE_FRAME_BYTES).unwrap();
        assert_eq!(decoded, message);
    }
}
