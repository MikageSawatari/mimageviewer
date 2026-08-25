//! mImageViewer 本体と remote-web 間の IPC プロトコル。
//!
//! GUI や Windows API には依存せず、型・版数・長さ付きフレームだけを共有する。

mod auth;
mod tailnet;
mod tailscale;

pub use auth::{
    AUTH_FILE_VERSION, AuthRecord, MAX_PIN_CHARS, MIN_PIN_CHARS, load_pin_file, production_argon2,
    rotate_session_secret_file, set_pin_file, validate_pin, validate_record,
};
pub use tailnet::{TailnetProbe, probe_tailnet};
pub use tailscale::{
    DEFAULT_REMOTE_PORT, TAILSCALE_COMMAND_TIMEOUT, TailscaleCommandError, TailscaleCommandOutput,
    run_tailscale, run_tailscale_at, tailscale_executable,
};

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
pub const PROTOCOL_VERSION: u32 = 52;
pub const MAX_CONTROL_FRAME_BYTES: usize = 128 * 1024;
pub const MAX_RESPONSE_FRAME_BYTES: usize = 64 * 1024 * 1024;
/// One wall-clock budget for the complete remote video start path, from core IPC queueing
/// through player/seek/encoder readiness and the first usable playlist.
pub const VIDEO_STREAM_START_BUDGET: Duration = Duration::from_secs(15);
/// A remote AI POST must be admitted by the core within this window. The job itself has no
/// inference deadline after admission.
pub const REMOTE_AI_START_ACCEPT_BUDGET: Duration = Duration::from_secs(2);
/// A remote archive POST only admits a long-running job. Inspection and conversion continue on
/// the core-owned job thread after this short IPC admission window.
pub const REMOTE_ARCHIVE_START_ACCEPT_BUDGET: Duration = Duration::from_secs(2);
pub const REMOTE_NETWORK_PATH_MESSAGE: &str = r"ネットワーク共有 (\\server\share 形式) はリモートからは開けません。ドライブ文字を割り当ててください。";

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
/// 実ファイル部分は絶対パスで表し、ZIP/PDF 内の位置だけを `subresource` へ追加する。
#[derive(Clone, Debug, Deserialize, Hash, PartialEq, Eq, Serialize)]
pub struct RemoteAddress {
    /// 対象の絶対パス。
    pub path: String,
    pub subresource: RemoteSubresource,
}

impl RemoteAddress {
    pub fn file(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            subresource: RemoteSubresource::File,
        }
    }

    /// トランスポート両端で共通に実行する構文検証。
    /// 実在確認と canonicalize は各プロセスが別途行う。
    pub fn validate_syntax(&self) -> Result<(), AddressError> {
        validate_absolute_path(&self.path)?;
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
    InvalidPath,
    NetworkPath,
    InvalidZipPath,
}

/// Core と remote-web が共有するアクセス境界の構文検証の正本。
///
/// 絶対パス判定と NUL 拒否を別の crate に複製せず、必ずこの関数を使うこと。
pub fn validate_absolute_path(value: &str) -> Result<(), AddressError> {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && matches!(bytes[0], b'/' | b'\\') && matches!(bytes[1], b'/' | b'\\') {
        return Err(AddressError::NetworkPath);
    }
    let path = std::path::Path::new(value);
    let absolute = path.is_absolute()
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'));
    if value.contains('\0') || !absolute {
        return Err(AddressError::InvalidPath);
    }
    Ok(())
}

fn validate_relative_component_path(value: &str, allow_empty: bool) -> Result<(), AddressError> {
    if (!allow_empty && value.is_empty())
        || value.contains('\0')
        || looks_absolute_or_drive_qualified(value)
        || value.split(['/', '\\']).any(|part| part == "..")
    {
        return Err(AddressError::InvalidPath);
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
    /// Identity of the item whose thumbnail is requested.
    pub address: RemoteAddress,
    /// Optional image source selected for the item (for example a video sidecar).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_address: Option<RemoteAddress>,
    pub target_px: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct FolderListRequest {
    pub address: RemoteAddress,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct FolderListEntry {
    /// Absolute filesystem address of the listed cell.
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
    /// 起点の種類を公開せずパンくず先頭を表示するための名前。
    pub root_name: String,
    pub thumb_aspect_height_ratio: f64,
    pub sort_state: RemoteGridSortState,
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
///
/// `SplitLtr` / `SplitRtl` は横長 1 ページを左右へ分けて 2 回の表示ステップとして読むモード。
/// 見開きとは**排他**で、`is_rtl` には含めない (見開きのペア並び順とは別の概念)。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSpreadMode {
    Single,
    Ltr,
    LtrCover,
    Rtl,
    RtlCover,
    SplitLtr,
    SplitRtl,
}

impl RemoteSpreadMode {
    pub fn is_rtl(self) -> bool {
        matches!(self, Self::Rtl | Self::RtlCover)
    }

    /// 横長ページを左右へ分割して読むモードか。
    pub fn is_split(self) -> bool {
        matches!(self, Self::SplitLtr | Self::SplitRtl)
    }
}

/// 分割したページの、どちら側を表示しているか。
///
/// **元ページのアドレスは変えない。** `PageGroup.anchor` は画像取得・履歴 URL・
/// キャッシュの identity として使われており、そこへ半分を埋め込むと URL と転送まで
/// 巻き込む。本体と同じく「元は正本、左右は表示だけ」に揃える。
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemotePageSlice {
    #[default]
    Full,
    Left,
    Right,
}

impl RemotePageSlice {
    /// 半分だけを表示しているか。
    pub fn is_half(self) -> bool {
        matches!(self, Self::Left | Self::Right)
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemotePostFilterOption {
    pub value: String,
    pub label: String,
    pub rewrites_pixels: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemotePostFilterGroup {
    pub label: String,
    pub options: Vec<RemotePostFilterOption>,
}

/// 選択肢と現在値。値・ラベル・分類は本体の `PostFilter` から毎回組み立てる。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemotePostFilterState {
    pub selected: String,
    pub groups: Vec<RemotePostFilterGroup>,
}

/// リモートの画像補正パネルが編集できるパラメータ。
///
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
    /// 旧 SPA の payload では欠落する。`None` はポストフィルタを変更しない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_filter: Option<String>,
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
    pub post_filter: RemotePostFilterState,
    /// `AiFeatureMode` を適用した後、effective params が final AI を一つ以上要求するか。
    pub effective_ai_enabled: bool,
    pub read_only: RemoteAdjustmentReadOnlyState,
}

/// 一覧で選べる値と現在値。値・ラベルは本体の `SortOrder` から毎回組み立てる。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteGridSortState {
    pub selected: String,
    pub options: Vec<RemoteGridSortOption>,
    /// `Some` の間は選択 UI を無効にし、同じ文言を利用者へ表示する。
    pub locked_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteGridSortOption {
    pub value: String,
    pub label: String,
    pub short_label: String,
}

/// 並べ替え操作の対象一覧。相互排他な scope を一つの型で所有する。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteGridScope {
    Address { address: RemoteAddress },
    Collection { collection: CollectionKind },
}

impl RemoteGridScope {
    pub fn address(&self) -> Option<&RemoteAddress> {
        match self {
            Self::Address { address } => Some(address),
            Self::Collection { .. } => None,
        }
    }

    pub fn address_mut(&mut self) -> Option<&mut RemoteAddress> {
        match self {
            Self::Address { address } => Some(address),
            Self::Collection { .. } => None,
        }
    }
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
    /// `state` は本体の `ViewTrimBookState` の serde 表現そのもの。
    /// IPC 専用のトリムモデルや既定値を持たず、本体 UI thread で型検証・clamp する。
    SetViewTrim {
        address: RemoteAddress,
        context_address: RemoteAddress,
        state: serde_json::Value,
    },
    GetViewTrimState {
        address: RemoteAddress,
        context_address: RemoteAddress,
    },
    SetSortOrder {
        scope: RemoteGridScope,
        /// 本体 `SortOrder` の serde 値。候補は `RemoteGridSortState` が返す。
        sort_order: String,
    },
}

impl RemoteWriteRequest {
    pub fn address(&self) -> Option<&RemoteAddress> {
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
            | Self::GetAdjustmentState { address }
            | Self::SetViewTrim { address, .. }
            | Self::GetViewTrimState { address, .. } => Some(address),
            Self::SetSortOrder { scope, .. } => scope.address(),
        }
    }

    pub fn address_mut(&mut self) -> Option<&mut RemoteAddress> {
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
            | Self::GetAdjustmentState { address }
            | Self::SetViewTrim { address, .. }
            | Self::GetViewTrimState { address, .. } => Some(address),
            Self::SetSortOrder { scope, .. } => scope.address_mut(),
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
            }
            | Self::SetViewTrim {
                context_address, ..
            }
            | Self::GetViewTrimState {
                context_address, ..
            } => Some(context_address),
            Self::SetSpread { .. }
            | Self::SetRating { .. }
            | Self::SetAdjustment { .. }
            | Self::GetAdjustmentState { .. }
            | Self::SetSortOrder { .. } => None,
        }
    }

    pub fn context_address_mut(&mut self) -> Option<&mut RemoteAddress> {
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
            }
            | Self::SetViewTrim {
                context_address, ..
            }
            | Self::GetViewTrimState {
                context_address, ..
            } => Some(context_address),
            Self::SetSpread { .. }
            | Self::SetRating { .. }
            | Self::SetAdjustment { .. }
            | Self::GetAdjustmentState { .. }
            | Self::SetSortOrder { .. } => None,
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
            Self::SetViewTrim { .. } => "set_view_trim",
            Self::GetViewTrimState { .. } => "get_view_trim_state",
            Self::SetSortOrder { .. } => "set_sort_order",
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
    pub view_trim_state: Option<serde_json::Value>,
    pub sort_state: Option<RemoteGridSortState>,
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
            view_trim_state: None,
            sort_state: None,
        }
    }

    pub fn adjustment_state(adjustment_state: RemoteAdjustmentState) -> Self {
        Self {
            item_state: None,
            adjustment_state: Some(adjustment_state),
            book_bookmarks: None,
            view_trim_state: None,
            sort_state: None,
        }
    }

    pub fn book_bookmarks(book_bookmarks: RemoteBookBookmarkList) -> Self {
        Self {
            item_state: None,
            adjustment_state: None,
            book_bookmarks: Some(book_bookmarks),
            view_trim_state: None,
            sort_state: None,
        }
    }

    pub fn view_trim_state(view_trim_state: serde_json::Value) -> Self {
        Self {
            view_trim_state: Some(view_trim_state),
            ..Self::default()
        }
    }

    pub fn sort_state(sort_state: RemoteGridSortState) -> Self {
        Self {
            sort_state: Some(sort_state),
            ..Self::default()
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
    ///
    /// **分割中は同じ anchor の group が 2 つ並ぶ**ので、これ単体では表示単位を
    /// 特定できない。位置の照合は `(anchor, slice)` の組で行うこと。
    pub anchor: RemoteAddress,
    pub pages: Vec<RemoteAddress>,
    /// 分割中にこの表示単位が元ページのどちら側か。分割していなければ `Full`。
    #[serde(default)]
    pub slice: RemotePageSlice,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContainerPayload {
    pub title: String,
    /// 起点の種類を公開せずパンくず先頭を表示するための名前。
    pub root_name: String,
    pub kind: ContainerKind,
    /// ZIP の単一ラッパー自動降下後など、実際に表示している位置。
    pub effective_address: RemoteAddress,
    pub entries: Vec<ContainerEntry>,
    /// ローカル一覧のサムネイルセルに使う高さ / 幅比。
    pub thumb_aspect_height_ratio: f64,
    pub sort_state: RemoteGridSortState,
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
    /// 本体 seek overlay と同じ nav item 分類による件数内訳。
    pub image_count: usize,
    pub video_count: usize,
    pub other_count: usize,
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
    /// Web の需要 coordinator が発行した、接続内で一意な page job identity。
    pub job_id: String,
    /// 先読みには表示要求がまだ無いため `None`。前景開始時だけ設定する。
    pub display_request_id: Option<String>,
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

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PageCancelCause {
    NoDemand,
    SessionInvalidated,
    ContextReset,
    ConnectionClosed,
    ServiceStopping,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PageDemandPromotion {
    pub job: String,
    /// 昇格の相関元。先読みの初回 PageRequest には display identity が無いため、
    /// 昇格操作そのものがこれを運ぶ。
    pub display: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PageDemandRelease {
    pub job: String,
    pub cause: PageCancelCause,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PageDemandRequest {
    pub promote: Vec<PageDemandPromotion>,
    pub release: Vec<PageDemandRelease>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PageDemandPromoteStatus {
    Promoted,
    AlreadyForeground,
    AlreadyReleased { cause: PageCancelCause },
    UnknownJob,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PageDemandPromoteResult {
    pub job: String,
    pub status: PageDemandPromoteStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PageDemandReleaseStatus {
    Released,
    AlreadyReleased { cause: PageCancelCause },
    Tombstoned,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PageDemandReleaseResult {
    pub job: String,
    pub status: PageDemandReleaseStatus,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PageDemandResponse {
    pub promote: Vec<PageDemandPromoteResult>,
    pub release: Vec<PageDemandReleaseResult>,
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
    Cancelled,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaceSummary {
    DriveList { name: String },
    ReadingHistory { name: String },
    Bookmarks { name: String },
    Rating { name: String, stars: Vec<u8> },
    Bookshelf { name: String },
    Separator,
    Folder { entry: RemoteEntry },
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
    /// Collection には永続化 key がないため、Web session 中の現在値だけを受け取る。
    pub spread_mode: Option<RemoteSpreadMode>,
    pub reading_direction: Option<RemoteReadingDirection>,
    /// 縦長 viewport 用の表示限定 Single。configured mode は変更しない。
    pub force_single_page: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CollectionKind {
    DriveList,
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
    /// 対象の絶対パス。
    pub path: String,
    /// Optional thumbnail source. Videos may point at a same-stem sidecar image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_address: Option<RemoteAddress>,
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
    pub sort_state: RemoteGridSortState,
    pub entries: Vec<RemoteEntry>,
    pub configured_spread_mode: RemoteSpreadMode,
    pub effective_spread_mode: RemoteSpreadMode,
    pub reading_direction: RemoteReadingDirection,
    pub image_count: usize,
    pub spread_page_gap_px: u32,
    /// Collection entry の index ではなく、コンテナと同じ address identity で表す。
    pub page_groups: Vec<PageGroup>,
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
pub struct FavoriteSearchRequest {
    pub query: String,
    pub kind: FavoriteSearchKind,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FavoriteSearchKind {
    All,
    Folder,
    Zip,
    Pdf,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FavoriteSearchPayload {
    pub listing: CollectionPayload,
    pub index_state: FavoriteSearchIndexState,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FavoriteSearchIndexState {
    Ready,
    Disabled,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum FavoriteSearchResponse {
    Success(FavoriteSearchPayload),
    Error(CollectionError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TagBrowseRequest;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteTagChoice {
    /// 表示名。画面上の `#` はクライアントが付ける。
    pub name: String,
    /// mIV 全体でこのタグが付いた項目数。
    pub count: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TagBrowsePayload {
    pub pinned: Vec<RemoteTagChoice>,
    pub recent: Vec<RemoteTagChoice>,
    pub popular: Vec<RemoteTagChoice>,
    /// 名前順の全タグ。端末内の絞り込みに使う。
    pub all: Vec<RemoteTagChoice>,
    pub all_truncated: bool,
    pub state: TagIndexState,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TagIndexState {
    Ready,
    Empty,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum TagBrowseResponse {
    Success(TagBrowsePayload),
    Error(CollectionError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TagItemsRequest {
    pub tag: String,
    pub kind: TagItemKind,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TagItemKind {
    All,
    Folder,
    Image,
    Video,
    Audio,
    Zip,
    Pdf,
    Archive,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TagItemsPayload {
    pub listing: CollectionPayload,
    pub state: TagIndexState,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum TagItemsResponse {
    Success(TagItemsPayload),
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

/// remote-web が起動時に確定した公開 URL と、その時点の接続準備状態。
///
/// 公開 URL は実際に配信している remote-web の snapshot として本体が表示する。
/// tailnet の現在状態を案内するときは `probe_tailnet` を使い、この snapshot を再利用しない。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteWebConnectionInfo {
    pub public_url: String,
    pub tailscale_serve: RemoteWebFeatureStatus,
    pub tailscale_serve_conflict: Option<String>,
    pub tailscale_serve_unsupported_path: Option<String>,
    pub tailscale_https_certificate: RemoteWebFeatureStatus,
    pub tailscale_key_expiry_unix_seconds: Option<i64>,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionReleaseRequest {
    pub owner: RemoteSessionIdentity,
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
    pub has_video: bool,
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
    pub has_video: bool,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteArchiveStartRequest {
    /// Client-generated idempotency key. Repeating it returns the original job.
    pub request_id: String,
    /// Public identity of the original archive. Cache paths are never accepted here.
    pub source: RemoteAddress,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteArchiveJobState {
    WaitingForLocalDrain,
    Inspecting,
    AwaitingConfirmation,
    AwaitingPassword,
    WaitingForConversionSlot,
    Converting,
    Finalizing,
    Cancelling,
    Ready,
    DeclinedByUser,
    Superseded,
    CancelledByUser,
    DiscardedByHost,
    BackgroundExpired,
    Failed,
}

impl RemoteArchiveJobState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Ready
                | Self::DeclinedByUser
                | Self::Superseded
                | Self::CancelledByUser
                | Self::DiscardedByHost
                | Self::BackgroundExpired
                | Self::Failed
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteArchiveProgress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_written: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteArchiveImageSummary {
    pub image_count: u32,
    pub total_uncompressed_bytes: u64,
    pub nested_archive_count: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteArchivePasswordResume {
    Inspect,
    Convert,
}

/// Input requested by an archive job. This contains only displayable metadata; the submitted
/// password is transported by `RemoteArchivePasswordRequest` and is never copied into a snapshot.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteArchiveAwaitingInput {
    Confirmation {
        summary: RemoteArchiveImageSummary,
    },
    Password {
        resume: RemoteArchivePasswordResume,
        bad_password: bool,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteArchiveTerminalCode {
    DeclinedByUser,
    Superseded,
    CancelledByUser,
    DiscardedByHost,
    BackgroundExpired,
    CacheUnavailable,
    PasswordUnsupported,
    SourceChanged,
    IgnoredBySettings,
    UnsupportedFormat,
    NoImages,
    ExecutionFailed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteArchiveTerminalDetail {
    pub code: RemoteArchiveTerminalCode,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteArchiveJobSnapshot {
    pub job_id: String,
    pub request_id: String,
    /// Always the original archive address, never a cache ZIP path.
    pub source: RemoteAddress,
    pub state: RemoteArchiveJobState,
    pub progress: Option<RemoteArchiveProgress>,
    pub awaiting_input: Option<RemoteArchiveAwaitingInput>,
    pub terminal: Option<RemoteArchiveTerminalDetail>,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteArchiveAccessMode {
    DirectRar,
    CachedZip,
}

/// Public result of preparing an archive. The core keeps the direct-RAR/cache-ZIP backing path in
/// its job registry; only the original source identity crosses IPC.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteArchiveOpenResult {
    pub source: RemoteAddress,
    pub access: RemoteArchiveAccessMode,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteArchiveJobErrorCode {
    BadRequest,
    StartExpired,
    SessionClosing,
    NotFound,
    Forbidden,
    JobGone,
    InvalidState,
    NotReady,
    Internal,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteArchiveJobError {
    pub code: RemoteArchiveJobErrorCode,
    pub message: String,
    pub terminal_code: Option<RemoteArchiveTerminalCode>,
}

impl RemoteArchiveJobError {
    pub fn new(code: RemoteArchiveJobErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            terminal_code: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteArchiveConfirmRequest {
    pub job_id: String,
    pub proceed: bool,
}

#[derive(Clone, Deserialize, PartialEq, Eq, Serialize)]
pub struct RemoteArchivePasswordRequest {
    pub job_id: String,
    pub password: String,
}

impl fmt::Debug for RemoteArchivePasswordRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteArchivePasswordRequest")
            .field("job_id", &self.job_id)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum RemoteArchiveStartResponse {
    Accepted(RemoteArchiveJobSnapshot),
    Error(RemoteArchiveJobError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum RemoteArchiveStateResponse {
    Success(RemoteArchiveJobSnapshot),
    Error(RemoteArchiveJobError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum RemoteArchiveRecoverableResponse {
    Success(Vec<RemoteArchiveJobSnapshot>),
    Error(RemoteArchiveJobError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum RemoteArchiveCancelResponse {
    Success(RemoteArchiveJobSnapshot),
    Error(RemoteArchiveJobError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum RemoteArchiveInputResponse {
    Success(RemoteArchiveJobSnapshot),
    Error(RemoteArchiveJobError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum RemoteArchiveResultResponse {
    Success(RemoteArchiveOpenResult),
    Error(RemoteArchiveJobError),
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
    SessionRelease {
        id: RequestId,
        request: SessionReleaseRequest,
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
    FavoriteSearch {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: FavoriteSearchRequest,
    },
    TagBrowse {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: TagBrowseRequest,
    },
    TagItems {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: TagItemsRequest,
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
    PageDemand {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: PageDemandRequest,
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
    RemoteArchiveStart {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: RemoteArchiveStartRequest,
        accept_before_unix_ms: u64,
    },
    RemoteArchiveState {
        id: RequestId,
        owner: RemoteSessionIdentity,
        job_id: String,
    },
    RemoteArchiveRecoverable {
        id: RequestId,
        owner: RemoteSessionIdentity,
    },
    RemoteArchiveCancel {
        id: RequestId,
        owner: RemoteSessionIdentity,
        job_id: String,
    },
    RemoteArchiveConfirm {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: RemoteArchiveConfirmRequest,
    },
    RemoteArchivePassword {
        id: RequestId,
        owner: RemoteSessionIdentity,
        request: RemoteArchivePasswordRequest,
    },
    RemoteArchiveResult {
        id: RequestId,
        owner: RemoteSessionIdentity,
        job_id: String,
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
        bar_width_px: Option<f64>,
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
            | Self::SessionRelease { id, .. }
            | Self::SessionActivity { id, .. }
            | Self::Thumbnail { id, .. }
            | Self::Home { id, .. }
            | Self::Collection { id, .. }
            | Self::FavoriteSearch { id, .. }
            | Self::TagBrowse { id, .. }
            | Self::TagItems { id, .. }
            | Self::FolderList { id, .. }
            | Self::Container { id, .. }
            | Self::Page { id, .. }
            | Self::PageDemand { id, .. }
            | Self::RemoteAiStart { id, .. }
            | Self::RemoteAiState { id, .. }
            | Self::RemoteAiRecoverable { id, .. }
            | Self::RemoteAiCancel { id, .. }
            | Self::RemoteAiResult { id, .. }
            | Self::RemoteArchiveStart { id, .. }
            | Self::RemoteArchiveState { id, .. }
            | Self::RemoteArchiveRecoverable { id, .. }
            | Self::RemoteArchiveCancel { id, .. }
            | Self::RemoteArchiveConfirm { id, .. }
            | Self::RemoteArchivePassword { id, .. }
            | Self::RemoteArchiveResult { id, .. }
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
    FavoriteSearch {
        id: RequestId,
        response: FavoriteSearchResponse,
    },
    TagBrowse {
        id: RequestId,
        response: TagBrowseResponse,
    },
    TagItems {
        id: RequestId,
        response: TagItemsResponse,
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
    PageDemand {
        id: RequestId,
        response: PageDemandResponse,
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
    RemoteArchiveStart {
        id: RequestId,
        response: RemoteArchiveStartResponse,
    },
    RemoteArchiveState {
        id: RequestId,
        response: RemoteArchiveStateResponse,
    },
    RemoteArchiveRecoverable {
        id: RequestId,
        response: RemoteArchiveRecoverableResponse,
    },
    RemoteArchiveCancel {
        id: RequestId,
        response: RemoteArchiveCancelResponse,
    },
    RemoteArchiveConfirm {
        id: RequestId,
        response: RemoteArchiveInputResponse,
    },
    RemoteArchivePassword {
        id: RequestId,
        response: RemoteArchiveInputResponse,
    },
    RemoteArchiveResult {
        id: RequestId,
        response: RemoteArchiveResultResponse,
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
            | Self::FavoriteSearch { id, .. }
            | Self::TagBrowse { id, .. }
            | Self::TagItems { id, .. }
            | Self::Container { id, .. }
            | Self::FolderList { id, .. }
            | Self::Page { id, .. }
            | Self::PageDemand { id, .. }
            | Self::RemoteAiStart { id, .. }
            | Self::RemoteAiState { id, .. }
            | Self::RemoteAiRecoverable { id, .. }
            | Self::RemoteAiCancel { id, .. }
            | Self::RemoteAiResult { id, .. }
            | Self::RemoteArchiveStart { id, .. }
            | Self::RemoteArchiveState { id, .. }
            | Self::RemoteArchiveRecoverable { id, .. }
            | Self::RemoteArchiveCancel { id, .. }
            | Self::RemoteArchiveConfirm { id, .. }
            | Self::RemoteArchivePassword { id, .. }
            | Self::RemoteArchiveResult { id, .. }
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
    NotReady,
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

    fn test_sort_state(locked_reason: Option<&str>) -> RemoteGridSortState {
        RemoteGridSortState {
            selected: "FileName".to_owned(),
            options: vec![RemoteGridSortOption {
                value: "FileName".to_owned(),
                label: "ファイル名順".to_owned(),
                short_label: "名前".to_owned(),
            }],
            locked_reason: locked_reason.map(str::to_owned),
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
            has_video: true,
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
    fn session_release_round_trips_the_exact_owner() {
        let expected = ClientMessage::SessionRelease {
            id: 10,
            request: SessionReleaseRequest {
                owner: test_owner("browser-instance"),
            },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        let actual: ClientMessage =
            read_frame(&mut bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn protocol_v52_connection_info_round_trips_with_tailnet_prerequisites_without_credentials() {
        assert_eq!(PROTOCOL_VERSION, 52);
        let expected = ClientMessage::RemoteWebConnectionInfo {
            id: 10,
            info: RemoteWebConnectionInfo {
                public_url: "https://viewer.example.ts.net/".to_owned(),
                tailscale_serve: RemoteWebFeatureStatus::NotConfigured,
                tailscale_serve_conflict: Some("http://127.0.0.1:3000".to_owned()),
                tailscale_serve_unsupported_path: Some("/miv".to_owned()),
                tailscale_https_certificate: RemoteWebFeatureStatus::Configured,
                tailscale_key_expiry_unix_seconds: Some(1_770_508_800),
            },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        let actual: ClientMessage =
            read_frame(&mut bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(actual, expected);
        let encoded = String::from_utf8(bytes[4..].to_vec()).unwrap();
        assert!(!encoded.contains("pin_configured"));
        assert!(!encoded.contains("pin="));
        assert!(!encoded.contains("bearer"));
    }

    #[test]
    fn thumbnail_message_round_trips_through_a_length_delimited_frame() {
        let expected = ClientMessage::Thumbnail {
            id: 42,
            owner: test_owner("test-client"),
            request: ThumbnailRequest {
                address: RemoteAddress::file("C:/Pictures/album/page.jpg"),
                source_address: Some(RemoteAddress::file("C:/Pictures/album/page-sidecar.jpg")),
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
                job_id: "page-job-43".to_owned(),
                display_request_id: None,
                address: RemoteAddress {
                    path: "C:/Books/volume.pdf".to_owned(),
                    subresource: RemoteSubresource::PdfPage { page_number: 7 },
                },
                target_px: 1805,
                priority: PagePriority::Prefetch,
                render_context: Some(RemotePageRenderContext {
                    context_address: RemoteAddress::file("C:/Books/volume.pdf"),
                    display_slot: RemotePageDisplaySlot::SpreadRight,
                    spread_partner: Some(RemoteAddress {
                        path: "C:/Books/volume.pdf".to_owned(),
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
                        post_filter: None,
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
    fn page_demand_round_trips_with_typed_results() {
        let request = ClientMessage::PageDemand {
            id: 44,
            owner: test_owner("test-client"),
            request: PageDemandRequest {
                promote: vec![PageDemandPromotion {
                    job: "page-job-43".to_owned(),
                    display: "display-9".to_owned(),
                }],
                release: vec![PageDemandRelease {
                    job: "page-job-41".to_owned(),
                    cause: PageCancelCause::NoDemand,
                }],
            },
        };
        let mut request_bytes = Vec::new();
        write_frame(&mut request_bytes, &request).unwrap();
        let actual_request: ClientMessage =
            read_frame(&mut request_bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(actual_request, request);

        let response = ServerMessage::PageDemand {
            id: 44,
            response: PageDemandResponse {
                promote: vec![PageDemandPromoteResult {
                    job: "page-job-43".to_owned(),
                    status: PageDemandPromoteStatus::Promoted,
                }],
                release: vec![PageDemandReleaseResult {
                    job: "page-job-41".to_owned(),
                    status: PageDemandReleaseStatus::Released,
                }],
            },
        };
        let mut response_bytes = Vec::new();
        write_frame(&mut response_bytes, &response).unwrap();
        let actual_response: ServerMessage =
            read_frame(&mut response_bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(actual_response, response);
    }

    #[test]
    fn folder_list_round_trips_with_sidecar_provenance() {
        let video = RemoteAddress::file("C:/Movies/sample.mp4");
        let sidecar = RemoteAddress::file("C:/Movies/sample.jpg");
        let request = ClientMessage::FolderList {
            id: 49,
            owner: test_owner("test-client"),
            request: FolderListRequest {
                address: RemoteAddress::file("C:/Movies"),
            },
        };
        let response = ServerMessage::FolderList {
            id: 49,
            response: FolderListResponse::Success(FolderListPayload {
                effective_address: RemoteAddress::file("C:/Movies"),
                root_name: "Fixture".to_owned(),
                thumb_aspect_height_ratio: 9.0 / 16.0,
                sort_state: test_sort_state(None),
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
    }

    #[test]
    fn protocol_v52_remote_video_thumbnail_shape_round_trips() {
        assert_eq!(PROTOCOL_VERSION, 52);
        let requests = [
            ClientMessage::VideoStreamStart {
                id: 50,
                owner: test_owner("test-client"),
                address: RemoteAddress::file("C:/Music/sample.flac"),
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
                bar_width_px: Some(360.0),
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
                    has_video: false,
                    encoder: "audio-only".to_owned(),
                    video_size: VideoStreamSize {
                        width: 0,
                        height: 0,
                    },
                    codecs: "mp4a.40.2".to_owned(),
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
        let container = RemoteAddress::file("C:/Books/book.pdf");
        let page = |page_number| RemoteAddress {
            path: "C:/Books/book.pdf".to_owned(),
            subresource: RemoteSubresource::PdfPage { page_number },
        };
        let expected = ServerMessage::Container {
            id: 44,
            response: ContainerResponse::Success(ContainerPayload {
                title: "book.pdf".to_owned(),
                root_name: "Fixture".to_owned(),
                kind: ContainerKind::Pdf,
                effective_address: container,
                entries: Vec::new(),
                thumb_aspect_height_ratio: 1.5,
                sort_state: test_sort_state(Some("本として表示中は名前順固定です")),
                resume_page: Some(page(1)),
                open_mode: ContainerOpenMode::ResumePage,
                configured_spread_mode: RemoteSpreadMode::RtlCover,
                effective_spread_mode: RemoteSpreadMode::RtlCover,
                reading_direction: RemoteReadingDirection::Rtl,
                image_count: 2,
                video_count: 0,
                other_count: 0,
                spread_page_gap_px: 8,
                page_groups: vec![PageGroup {
                    anchor: page(0),
                    pages: vec![page(1), page(0)],
                    slice: RemotePageSlice::Full,
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
            address: RemoteAddress::file("C:/Books/book.pdf"),
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
                address: RemoteAddress::file("C:/Books/book.pdf"),
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
        let container = RemoteAddress::file("C:/Books/book.pdf");
        let page = RemoteAddress {
            path: "C:/Books/book.pdf".to_owned(),
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
                    post_filter: Some("crt_simple".to_owned()),
                    ai: Some(RemoteAiAdjustmentValues {
                        upscale_model: Some("auto".to_owned()),
                        denoise_model: None,
                    }),
                },
            },
            RemoteWriteRequest::GetAdjustmentState {
                address: page.clone(),
            },
            RemoteWriteRequest::SetViewTrim {
                address: page.clone(),
                context_address: RemoteAddress::file("C:/Books/book.pdf"),
                state: serde_json::json!({
                    "apply_mode": "book",
                    "book_settings": {
                        "enabled": true,
                        "spread_separate": false,
                        "single": { "left": 0.01, "top": 0.02, "right": 0.03, "bottom": 0.04 },
                        "spread_linked": { "top": 0.01, "bottom": 0.02, "inner": 0.03, "outer": 0.04 },
                        "spread_left": { "left": 0.0, "top": 0.0, "right": 0.0, "bottom": 0.0 },
                        "spread_right": { "left": 0.0, "top": 0.0, "right": 0.0, "bottom": 0.0 }
                    }
                }),
            },
            RemoteWriteRequest::GetViewTrimState {
                address: page,
                context_address: RemoteAddress::file("C:/Books/book.pdf"),
            },
            RemoteWriteRequest::SetSortOrder {
                scope: RemoteGridScope::Collection {
                    collection: CollectionKind::SmartFolder {
                        definition_id: "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2".to_owned(),
                    },
                },
                sort_order: "DateDesc".to_owned(),
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
                            path: "C:/Books/book.zip".to_owned(),
                            subresource: RemoteSubresource::ZipEntry {
                                entry_name: "chapter/009.jpg".to_owned(),
                            },
                        },
                        context_address: RemoteAddress {
                            path: "C:/Books/book.zip".to_owned(),
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
                    post_filter: Some("crt_simple".to_owned()),
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
                    post_filter: Some("none".to_owned()),
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
                post_filter: RemotePostFilterState {
                    selected: "crt_simple".to_owned(),
                    groups: vec![RemotePostFilterGroup {
                        label: "基本".to_owned(),
                        options: vec![RemotePostFilterOption {
                            value: "none".to_owned(),
                            label: "標準（補間あり）".to_owned(),
                            rewrites_pixels: false,
                        }],
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
    fn collection_message_round_trip_keeps_spread_request_and_address_groups() {
        let request = ClientMessage::Collection {
            id: 77,
            owner: test_owner("test-client"),
            request: CollectionRequest {
                kind: CollectionKind::SmartFolder {
                    definition_id: "00000000-0000-0000-0000-000000000001".to_owned(),
                },
                spread_mode: Some(RemoteSpreadMode::RtlCover),
                reading_direction: Some(RemoteReadingDirection::Rtl),
                force_single_page: false,
            },
        };
        let page = RemoteAddress::file("C:/Books/volume-1/page-001.jpg");
        let expected = ServerMessage::Collection {
            id: 77,
            response: CollectionResponse::Success(CollectionPayload {
                title: "最近読んだ本".to_owned(),
                thumb_aspect_height_ratio: 1.0,
                sort_state: test_sort_state(Some("この一覧では並び順が固定されています")),
                configured_spread_mode: RemoteSpreadMode::RtlCover,
                effective_spread_mode: RemoteSpreadMode::RtlCover,
                reading_direction: RemoteReadingDirection::Rtl,
                image_count: 1,
                spread_page_gap_px: 4,
                page_groups: vec![PageGroup {
                    anchor: page.clone(),
                    pages: vec![page],
                    slice: RemotePageSlice::Full,
                }],
                entry_limit: 1000,
                truncated: false,
                entries: vec![RemoteEntry {
                    path: "C:/Books/volume-1/page-001.jpg".to_owned(),
                    thumbnail_address: None,
                    name: "page-001.jpg".to_owned(),
                    kind: RemoteEntryKind::Image,
                    detail: Some("3 / 20 ページ".to_owned()),
                    progress_current: Some(3),
                    progress_total: Some(20),
                    rating: None,
                }],
            }),
        };
        let mut request_bytes = Vec::new();
        write_frame(&mut request_bytes, &request).unwrap();
        let decoded_request: ClientMessage =
            read_frame(&mut request_bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(decoded_request, request);
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        let decoded: ServerMessage =
            read_frame(&mut bytes.as_slice(), MAX_RESPONSE_FRAME_BYTES).unwrap();
        assert_eq!(decoded, expected);
        let encoded = String::from_utf8(bytes[4..].to_vec()).unwrap();
        assert!(!encoded.contains(r"C:\\"));
    }

    #[test]
    fn favorite_search_message_round_trip_keeps_the_listing_shape() {
        let request = ClientMessage::FavoriteSearch {
            id: 78,
            owner: test_owner("test-client"),
            request: FavoriteSearchRequest {
                query: "album -draft".to_owned(),
                kind: FavoriteSearchKind::Pdf,
            },
        };
        let response = ServerMessage::FavoriteSearch {
            id: 78,
            response: FavoriteSearchResponse::Success(FavoriteSearchPayload {
                listing: CollectionPayload {
                    title: "検索結果".to_owned(),
                    thumb_aspect_height_ratio: 1.0,
                    sort_state: test_sort_state(Some("この一覧では並び順が固定されています")),
                    entries: Vec::new(),
                    configured_spread_mode: RemoteSpreadMode::Single,
                    effective_spread_mode: RemoteSpreadMode::Single,
                    reading_direction: RemoteReadingDirection::Ltr,
                    image_count: 0,
                    spread_page_gap_px: 4,
                    page_groups: Vec::new(),
                    entry_limit: 1000,
                    truncated: false,
                },
                index_state: FavoriteSearchIndexState::Ready,
            }),
        };
        let mut request_bytes = Vec::new();
        write_frame(&mut request_bytes, &request).unwrap();
        let decoded_request: ClientMessage =
            read_frame(&mut request_bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(decoded_request, request);
        let mut response_bytes = Vec::new();
        write_frame(&mut response_bytes, &response).unwrap();
        let decoded_response: ServerMessage =
            read_frame(&mut response_bytes.as_slice(), MAX_RESPONSE_FRAME_BYTES).unwrap();
        assert_eq!(decoded_response, response);
    }

    #[test]
    fn tag_browse_and_items_messages_round_trip() {
        let browse = ClientMessage::TagBrowse {
            id: 79,
            owner: test_owner("test-client"),
            request: TagBrowseRequest,
        };
        let items = ClientMessage::TagItems {
            id: 80,
            owner: test_owner("test-client"),
            request: TagItemsRequest {
                tag: "風景".to_owned(),
                kind: TagItemKind::Image,
            },
        };
        let browse_response = ServerMessage::TagBrowse {
            id: 79,
            response: TagBrowseResponse::Success(TagBrowsePayload {
                pinned: vec![RemoteTagChoice {
                    name: "風景".to_owned(),
                    count: 3,
                }],
                recent: Vec::new(),
                popular: Vec::new(),
                all: vec![RemoteTagChoice {
                    name: "風景".to_owned(),
                    count: 3,
                }],
                all_truncated: false,
                state: TagIndexState::Ready,
            }),
        };
        let items_response = ServerMessage::TagItems {
            id: 80,
            response: TagItemsResponse::Success(TagItemsPayload {
                listing: CollectionPayload {
                    title: "タグの項目".to_owned(),
                    thumb_aspect_height_ratio: 1.0,
                    sort_state: test_sort_state(Some("この一覧では並び順が固定されています")),
                    entries: Vec::new(),
                    configured_spread_mode: RemoteSpreadMode::Single,
                    effective_spread_mode: RemoteSpreadMode::Single,
                    reading_direction: RemoteReadingDirection::Ltr,
                    image_count: 0,
                    spread_page_gap_px: 4,
                    page_groups: Vec::new(),
                    entry_limit: 1000,
                    truncated: false,
                },
                state: TagIndexState::Ready,
            }),
        };

        for request in [browse, items] {
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &request).unwrap();
            let decoded: ClientMessage =
                read_frame(&mut bytes.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
            assert_eq!(decoded, request);
        }
        for response in [browse_response, items_response] {
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &response).unwrap();
            let decoded: ServerMessage =
                read_frame(&mut bytes.as_slice(), MAX_RESPONSE_FRAME_BYTES).unwrap();
            assert_eq!(decoded, response);
        }
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
    fn absolute_path_boundary_is_shared_and_rejects_network_namespaces_and_nul() {
        for path in [r"C:\Pictures\page.jpg", "Z:/Pictures/page.jpg"] {
            validate_absolute_path(path).unwrap();
        }
        for path in ["relative/page.jpg", "C:page.jpg", "C:/bad\0page.jpg"] {
            assert_eq!(validate_absolute_path(path), Err(AddressError::InvalidPath));
        }
        for path in [
            r"\\nas\share\a.jpg",
            "//nas/share/a.jpg",
            r"\\?\C:\a.jpg",
            r"\\.\PhysicalDrive0",
            r"\/nas/share/a.jpg",
            r"/\nas\share\a.jpg",
        ] {
            assert_eq!(
                validate_absolute_path(path),
                Err(AddressError::NetworkPath),
                "accepted {path:?}"
            );
        }
    }

    #[test]
    fn place_summaries_keep_order_separators_and_typed_folder_targets() {
        let places = vec![
            PlaceSummary::DriveList {
                name: "ドライブ一覧".to_owned(),
            },
            PlaceSummary::Rating {
                name: "レーティング".to_owned(),
                stars: vec![1, 2, 3, 4, 5],
            },
            PlaceSummary::Separator,
            PlaceSummary::Folder {
                entry: RemoteEntry {
                    path: "C:/Users/test/Pictures".to_owned(),
                    thumbnail_address: None,
                    name: "ピクチャ".to_owned(),
                    kind: RemoteEntryKind::Folder,
                    detail: None,
                    progress_current: None,
                    progress_total: None,
                    rating: None,
                },
            },
        ];
        let encoded = serde_json::to_vec(&places).unwrap();
        let decoded: Vec<PlaceSummary> = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, places);
        let json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(json[0]["kind"], "drive_list");
        assert_eq!(json[1]["stars"], serde_json::json!([1, 2, 3, 4, 5]));
        assert_eq!(json[2]["kind"], "separator");
        assert_eq!(json[3]["entry"]["kind"], "folder");
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
                path: "C:/Books/volume.zip".to_owned(),
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
                path: "C:/Books/volume.zip".to_owned(),
                subresource: RemoteSubresource::ZipEntry {
                    entry_name: "chapter.zip/pages/001.jpg".to_owned(),
                },
            },
            RemoteAddress {
                path: "C:/Books/volume.pdf".to_owned(),
                subresource: RemoteSubresource::PdfPage { page_number: 42 },
            },
        ];
        for address in addresses {
            address.validate_syntax().unwrap();
            let encoded = serde_json::to_vec(&address).unwrap();
            let decoded: RemoteAddress = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(decoded, address);
            assert!(String::from_utf8(encoded).unwrap().contains("C:/Books/"));
        }
    }

    #[test]
    fn page_payload_carries_the_rendered_identity_across_ipc() {
        let identity = RemoteAddress {
            path: "C:/Books/volume.pdf".to_owned(),
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
        let address = RemoteAddress::file("C:/Pages/001.png");
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

    #[test]
    fn remote_archive_password_is_redacted_and_never_enters_snapshot() {
        let password = RemoteArchivePasswordRequest {
            job_id: "archive-7-1".to_owned(),
            password: "super-secret".to_owned(),
        };
        let debug = format!("{password:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("super-secret"));

        let message = ClientMessage::RemoteArchivePassword {
            id: 41,
            owner: test_owner("phone"),
            request: password.clone(),
        };
        let message_debug = format!("{message:?}");
        assert!(!message_debug.contains("super-secret"));
        let mut frame = Vec::new();
        write_frame(&mut frame, &message).unwrap();
        let decoded: ClientMessage =
            read_frame(&mut frame.as_slice(), MAX_CONTROL_FRAME_BYTES).unwrap();
        assert_eq!(decoded, message);

        let snapshot = RemoteArchiveJobSnapshot {
            job_id: password.job_id.clone(),
            request_id: "request-1".to_owned(),
            source: RemoteAddress::file("C:/Books/secret.rar"),
            state: RemoteArchiveJobState::AwaitingPassword,
            progress: None,
            awaiting_input: Some(RemoteArchiveAwaitingInput::Password {
                resume: RemoteArchivePasswordResume::Convert,
                bad_password: true,
            }),
            terminal: None,
            created_unix_ms: 10,
            updated_unix_ms: 20,
        };
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.contains("super-secret"));
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert!(value.get("password").is_none());
    }
}
