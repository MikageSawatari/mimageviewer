//! Ring shortcut settings and action identifiers.
//!
//! The input state machines and command execution live in later phases. This
//! module keeps the persisted action ids, context filtering, and defaults in one
//! place so preferences UI and future input dispatch use the same inventory.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use serde::{Deserialize, Serialize};

pub const RING_SHORTCUT_SLOT_COUNT: usize = 8;
pub const MOUSE_FLICK_MOVE_THRESHOLD_PX: f32 = 20.0;
pub const MOUSE_FLICK_NEUTRAL_RADIUS_PX: f32 = 48.0;
pub const MOUSE_FLICK_GUIDE_DELAY_MS: u64 = 170;
pub const MOUSE_FLICK_MENU_DELAY_MS: u64 = 400;

pub fn mouse_flick_guide_delay() -> Duration {
    Duration::from_millis(MOUSE_FLICK_GUIDE_DELAY_MS)
}

pub fn mouse_flick_menu_delay() -> Duration {
    Duration::from_millis(MOUSE_FLICK_MENU_DELAY_MS)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RingShortcutContext {
    Grid,
    ImageFullscreen,
    VideoFullscreen,
}

impl RingShortcutContext {
    pub fn all() -> &'static [Self] {
        const ALL: [RingShortcutContext; 3] = [
            RingShortcutContext::Grid,
            RingShortcutContext::ImageFullscreen,
            RingShortcutContext::VideoFullscreen,
        ];
        &ALL
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Grid => "グリッド",
            Self::ImageFullscreen => "画像フルスクリーン",
            Self::VideoFullscreen => "動画フルスクリーン",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RightDragContext {
    Grid,
    ImageFullscreen,
    VideoFullscreen,
    EditMode,
}

impl RightDragContext {
    pub fn all() -> &'static [Self] {
        const ALL: [RightDragContext; 4] = [
            RightDragContext::Grid,
            RightDragContext::ImageFullscreen,
            RightDragContext::VideoFullscreen,
            RightDragContext::EditMode,
        ];
        &ALL
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Grid => "グリッド",
            Self::ImageFullscreen => "画像フルスクリーン",
            Self::VideoFullscreen => "動画フルスクリーン",
            Self::EditMode => "編集モード",
        }
    }

    pub fn ring_context(self) -> Option<RingShortcutContext> {
        match self {
            Self::Grid => Some(RingShortcutContext::Grid),
            Self::ImageFullscreen => Some(RingShortcutContext::ImageFullscreen),
            Self::VideoFullscreen => Some(RingShortcutContext::VideoFullscreen),
            Self::EditMode => None,
        }
    }

    pub fn gesture_action_context(self) -> RingShortcutContext {
        match self {
            Self::Grid => RingShortcutContext::Grid,
            Self::ImageFullscreen | Self::EditMode => RingShortcutContext::ImageFullscreen,
            Self::VideoFullscreen => RingShortcutContext::VideoFullscreen,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RightDragMode {
    Disabled,
    RingShortcut,
    MouseGesture,
    Unknown(String),
}

impl RightDragMode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Disabled => "disabled",
            Self::RingShortcut => "ring_shortcut",
            Self::MouseGesture => "mouse_gesture",
            Self::Unknown(s) => s.as_str(),
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "disabled" | "none" | "" => Self::Disabled,
            "ring_shortcut" | "ring" => Self::RingShortcut,
            "mouse_gesture" | "gesture" => Self::MouseGesture,
            _ => return None,
        })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Disabled => "未使用",
            Self::RingShortcut => "リングショートカット",
            Self::MouseGesture => "マウスジェスチャ",
            Self::Unknown(_) => "不明な設定",
        }
    }

    pub fn effective(&self) -> Self {
        match self {
            Self::Disabled => Self::Disabled,
            Self::RingShortcut => Self::RingShortcut,
            Self::MouseGesture => Self::MouseGesture,
            Self::Unknown(_) => Self::Disabled,
        }
    }
}

impl Default for RightDragMode {
    fn default() -> Self {
        Self::Disabled
    }
}

impl Serialize for RightDragMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RightDragMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_str(&value).unwrap_or(Self::Unknown(value)))
    }
}
pub const MOUSE_GESTURE_MAX_STROKES: usize = 4;
pub const MOUSE_GESTURE_STEP_THRESHOLD_PX: f32 = 36.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseGestureDirection {
    Up,
    Right,
    Down,
    Left,
}

impl MouseGestureDirection {
    pub fn all() -> &'static [Self] {
        const ALL: [MouseGestureDirection; 4] = [
            MouseGestureDirection::Up,
            MouseGestureDirection::Right,
            MouseGestureDirection::Down,
            MouseGestureDirection::Left,
        ];
        &ALL
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Right => "right",
            Self::Down => "down",
            Self::Left => "left",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "u" | "up" | "↑" | "上" => Self::Up,
            "r" | "right" | "→" | "右" => Self::Right,
            "d" | "down" | "↓" | "下" => Self::Down,
            "l" | "left" | "←" | "左" => Self::Left,
            _ => return None,
        })
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Up => "↑",
            Self::Right => "→",
            Self::Down => "↓",
            Self::Left => "←",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Up => "↑ 上",
            Self::Right => "→ 右",
            Self::Down => "↓ 下",
            Self::Left => "← 左",
        }
    }
}

impl Serialize for MouseGestureDirection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MouseGestureDirection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value)
            .ok_or_else(|| serde::de::Error::custom("unknown mouse gesture direction"))
    }
}

pub fn format_mouse_gesture_pattern(pattern: &[MouseGestureDirection]) -> String {
    pattern
        .iter()
        .map(|d| d.symbol())
        .collect::<Vec<_>>()
        .join("")
}

pub fn parse_mouse_gesture_pattern(input: &str) -> Option<Vec<MouseGestureDirection>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = trimmed
        .replace('＞', ">")
        .replace('，', ",")
        .replace('、', ",")
        .replace('-', " ")
        .replace('>', " ")
        .replace(',', " ")
        .replace('/', " ");
    let mut out = Vec::new();
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    if tokens.len() > 1 {
        for token in tokens {
            out.push(MouseGestureDirection::from_str(token)?);
        }
    } else if let Some(token) = tokens.first()
        && let Some(direction) = MouseGestureDirection::from_str(token)
    {
        out.push(direction);
    } else {
        for ch in normalized.chars().filter(|c| !c.is_whitespace()) {
            out.push(MouseGestureDirection::from_str(&ch.to_string())?);
        }
    }
    normalize_mouse_gesture_pattern(out)
}

pub fn normalize_mouse_gesture_pattern(
    pattern: Vec<MouseGestureDirection>,
) -> Option<Vec<MouseGestureDirection>> {
    let mut out = Vec::new();
    for direction in pattern {
        if out.last().copied() == Some(direction) {
            continue;
        }
        if out.len() >= MOUSE_GESTURE_MAX_STROKES {
            return None;
        }
        out.push(direction);
    }
    (!out.is_empty()).then_some(out)
}

fn deserialize_mouse_gesture_pattern<'de, D>(
    deserializer: D,
) -> Result<Vec<MouseGestureDirection>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<String>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .filter_map(|value| MouseGestureDirection::from_str(&value))
        .collect())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MouseGestureBinding {
    #[serde(default, deserialize_with = "deserialize_mouse_gesture_pattern")]
    pub pattern: Vec<MouseGestureDirection>,
    #[serde(default)]
    pub action: RingActionId,
}

impl MouseGestureBinding {
    pub fn new(pattern: Vec<MouseGestureDirection>, action: RingActionId) -> Self {
        Self { pattern, action }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MouseGestureProfile {
    #[serde(default)]
    pub bindings: Vec<MouseGestureBinding>,
}

impl MouseGestureProfile {
    pub fn sanitize(&mut self, context: RightDragContext) {
        let action_context = context.gesture_action_context();
        let mut sanitized = Vec::new();
        for mut binding in self.bindings.drain(..) {
            let Some(pattern) = normalize_mouse_gesture_pattern(binding.pattern) else {
                continue;
            };
            binding.pattern = pattern;
            if !binding.action.is_valid_for_context(action_context) {
                binding.action = RingActionId::None;
            }
            sanitized.push(binding);
        }
        self.bindings = sanitized;
    }

    pub fn action_for_pattern(&self, pattern: &[MouseGestureDirection]) -> RingActionId {
        self.bindings
            .iter()
            .find(|binding| binding.pattern == pattern)
            .map(|binding| binding.action.clone())
            .unwrap_or(RingActionId::None)
    }
}

impl Default for MouseGestureProfile {
    fn default() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RingDirection {
    Up,
    UpRight,
    Right,
    DownRight,
    Down,
    DownLeft,
    Left,
    UpLeft,
}

impl RingDirection {
    pub fn all() -> &'static [Self] {
        const ALL: [RingDirection; RING_SHORTCUT_SLOT_COUNT] = [
            RingDirection::Up,
            RingDirection::UpRight,
            RingDirection::Right,
            RingDirection::DownRight,
            RingDirection::Down,
            RingDirection::DownLeft,
            RingDirection::Left,
            RingDirection::UpLeft,
        ];
        &ALL
    }

    pub fn slot_index(self) -> usize {
        match self {
            Self::Up => 0,
            Self::UpRight => 1,
            Self::Right => 2,
            Self::DownRight => 3,
            Self::Down => 4,
            Self::DownLeft => 5,
            Self::Left => 6,
            Self::UpLeft => 7,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Up => "↑ 上",
            Self::UpRight => "↗ 右上",
            Self::Right => "→ 右",
            Self::DownRight => "↘ 右下",
            Self::Down => "↓ 下",
            Self::DownLeft => "↙ 左下",
            Self::Left => "← 左",
            Self::UpLeft => "↖ 左上",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RingActionId {
    None,
    AddToBook,
    PinRepresentativeThumb,
    ToggleDetachedViewer,
    ToggleWindowMode,
    ToggleMaximize,
    CycleFavorite,
    GridToggleDetails,
    GridToggleSnapshotLock,
    GridToggleCheck,
    GridSelectAll,
    GridHistoryBack,
    GridHistoryForward,
    GridParentFolder,
    TreeFolderPrev,
    TreeFolderNext,
    SiblingFolderPrev,
    SiblingFolderNext,
    ImageHome,
    ImageEnd,
    ImageRotateLeft,
    ImageRotateRight,
    ImageCapture,
    ImageToggleMetadata,
    ImageSlideshow,
    ImagePixelGrid,
    ImageBackgroundCycle,
    ImageComparePin,
    ImageCopyToClipboard,
    ImageOpenFolder,
    ImageCopyPath,
    ImageCopyFileName,
    VideoCapture,
    VideoMute,
    VideoLoop,
    VideoBookmark,
    VideoMarkerPrev,
    VideoMarkerNext,
    VideoTileMode,
    VideoExternalPlayer,
    Unknown(String),
}

impl RingActionId {
    pub fn as_str(&self) -> &str {
        match self {
            Self::None => "none",
            Self::AddToBook => "add_to_book",
            Self::PinRepresentativeThumb => "pin_representative_thumb",
            Self::ToggleDetachedViewer => "toggle_detached_viewer",
            Self::ToggleWindowMode => "toggle_window_mode",
            Self::ToggleMaximize => "toggle_maximize",
            Self::CycleFavorite => "cycle_favorite",
            Self::GridToggleDetails => "grid_toggle_details",
            Self::GridToggleSnapshotLock => "grid_toggle_snapshot_lock",
            Self::GridToggleCheck => "grid_toggle_check",
            Self::GridSelectAll => "grid_select_all",
            Self::GridHistoryBack => "grid_history_back",
            Self::GridHistoryForward => "grid_history_forward",
            Self::GridParentFolder => "grid_parent_folder",
            Self::TreeFolderPrev => "tree_folder_prev",
            Self::TreeFolderNext => "tree_folder_next",
            Self::SiblingFolderPrev => "sibling_folder_prev",
            Self::SiblingFolderNext => "sibling_folder_next",
            Self::ImageHome => "image_home",
            Self::ImageEnd => "image_end",
            Self::ImageRotateLeft => "image_rotate_left",
            Self::ImageRotateRight => "image_rotate_right",
            Self::ImageCapture => "image_capture",
            Self::ImageToggleMetadata => "image_toggle_metadata",
            Self::ImageSlideshow => "image_slideshow",
            Self::ImagePixelGrid => "image_pixel_grid",
            Self::ImageBackgroundCycle => "image_background_cycle",
            Self::ImageComparePin => "image_compare_pin",
            Self::ImageCopyToClipboard => "image_copy_to_clipboard",
            Self::ImageOpenFolder => "image_open_folder",
            Self::ImageCopyPath => "image_copy_path",
            Self::ImageCopyFileName => "image_copy_file_name",
            Self::VideoCapture => "video_capture",
            Self::VideoMute => "video_mute",
            Self::VideoLoop => "video_loop",
            Self::VideoBookmark => "video_bookmark",
            Self::VideoMarkerPrev => "video_marker_prev",
            Self::VideoMarkerNext => "video_marker_next",
            Self::VideoTileMode => "video_tile_mode",
            Self::VideoExternalPlayer => "video_external_player",
            Self::Unknown(s) => s.as_str(),
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "none" | "" => Self::None,
            "add_to_book" => Self::AddToBook,
            "pin_representative_thumb" => Self::PinRepresentativeThumb,
            "toggle_detached_viewer" => Self::ToggleDetachedViewer,
            "toggle_window_mode" => Self::ToggleWindowMode,
            "toggle_maximize" => Self::ToggleMaximize,
            "cycle_favorite" => Self::CycleFavorite,
            "grid_toggle_details" => Self::GridToggleDetails,
            "grid_toggle_snapshot_lock" => Self::GridToggleSnapshotLock,
            "grid_toggle_check" => Self::GridToggleCheck,
            "grid_select_all" => Self::GridSelectAll,
            "grid_history_back" => Self::GridHistoryBack,
            "grid_history_forward" => Self::GridHistoryForward,
            "grid_parent_folder" => Self::GridParentFolder,
            "tree_folder_prev" => Self::TreeFolderPrev,
            "tree_folder_next" => Self::TreeFolderNext,
            "sibling_folder_prev" => Self::SiblingFolderPrev,
            "sibling_folder_next" => Self::SiblingFolderNext,
            "image_home" => Self::ImageHome,
            "image_end" => Self::ImageEnd,
            "image_rotate_left" => Self::ImageRotateLeft,
            "image_rotate_right" => Self::ImageRotateRight,
            "image_capture" => Self::ImageCapture,
            "image_toggle_metadata" => Self::ImageToggleMetadata,
            "image_slideshow" => Self::ImageSlideshow,
            "image_pixel_grid" => Self::ImagePixelGrid,
            "image_background_cycle" => Self::ImageBackgroundCycle,
            "image_compare_pin" => Self::ImageComparePin,
            "image_copy_to_clipboard" => Self::ImageCopyToClipboard,
            "image_open_folder" => Self::ImageOpenFolder,
            "image_copy_path" => Self::ImageCopyPath,
            "image_copy_file_name" => Self::ImageCopyFileName,
            "video_capture" => Self::VideoCapture,
            "video_mute" => Self::VideoMute,
            "video_loop" => Self::VideoLoop,
            "video_bookmark" => Self::VideoBookmark,
            "video_marker_prev" => Self::VideoMarkerPrev,
            "video_marker_next" => Self::VideoMarkerNext,
            "video_tile_mode" => Self::VideoTileMode,
            "video_external_player" => Self::VideoExternalPlayer,
            _ => return None,
        })
    }

    pub fn label_for_context(&self, context: RingShortcutContext) -> &'static str {
        match self {
            Self::None => "なし",
            Self::AddToBook => match context {
                RingShortcutContext::VideoFullscreen => "本棚に追加 (フレーム)",
                _ => "本棚に追加",
            },
            Self::PinRepresentativeThumb => match context {
                RingShortcutContext::VideoFullscreen => "代表フレームにピン留め",
                _ => "代表サムネにピン留め",
            },
            Self::ToggleDetachedViewer => "別ウィンドウ ON/OFF",
            Self::ToggleWindowMode => "ウィンドウ/全画面切替",
            Self::ToggleMaximize => "ウィンドウ最大化/復元",
            Self::CycleFavorite => "お気に入り巡回",
            Self::GridToggleDetails => "表示/詳細",
            Self::GridToggleSnapshotLock => "★固定",
            Self::GridToggleCheck => "チェック ON/OFF",
            Self::GridSelectAll => "全選択",
            Self::GridHistoryBack => "フォルダ履歴 戻る",
            Self::GridHistoryForward => "フォルダ履歴 進む",
            Self::GridParentFolder => "親フォルダへ",
            Self::TreeFolderPrev => "ツリー順 前フォルダ",
            Self::TreeFolderNext => "ツリー順 次フォルダ",
            Self::SiblingFolderPrev => "兄弟フォルダ 前",
            Self::SiblingFolderNext => "兄弟フォルダ 次",
            Self::ImageHome => "先頭へ (Home)",
            Self::ImageEnd => "末尾へ (End)",
            Self::ImageRotateLeft => "回転 L",
            Self::ImageRotateRight => "回転 R",
            Self::ImageCapture => "キャプチャ保存",
            Self::ImageToggleMetadata => "メタデータ表示",
            Self::ImageSlideshow => "スライドショー",
            Self::ImagePixelGrid => "ピクセルグリッド",
            Self::ImageBackgroundCycle => "背景色サイクル",
            Self::ImageComparePin => "比較ピン",
            Self::ImageCopyToClipboard => "画像をクリップボードにコピー",
            Self::ImageOpenFolder => "フォルダを開く",
            Self::ImageCopyPath => "パスをコピー",
            Self::ImageCopyFileName => "ファイル名をコピー",
            Self::VideoCapture => "キャプチャ保存",
            Self::VideoMute => "ミュート",
            Self::VideoLoop => "ループ",
            Self::VideoBookmark => "ブックマーク追加",
            Self::VideoMarkerPrev => "前のマーカー",
            Self::VideoMarkerNext => "次のマーカー",
            Self::VideoTileMode => "タイルモード",
            Self::VideoExternalPlayer => "外部プレイヤーで開く",
            Self::Unknown(_) => "不明なアクション",
        }
    }

    pub fn is_valid_for_context(&self, context: RingShortcutContext) -> bool {
        match context {
            RingShortcutContext::Grid => matches!(
                self,
                Self::None
                    | Self::ToggleMaximize
                    | Self::AddToBook
                    | Self::PinRepresentativeThumb
                    | Self::CycleFavorite
                    | Self::GridToggleDetails
                    | Self::GridToggleSnapshotLock
                    | Self::GridToggleCheck
                    | Self::GridSelectAll
                    | Self::GridHistoryBack
                    | Self::GridHistoryForward
                    | Self::GridParentFolder
                    | Self::TreeFolderPrev
                    | Self::TreeFolderNext
                    | Self::SiblingFolderPrev
                    | Self::SiblingFolderNext
            ),
            RingShortcutContext::ImageFullscreen => matches!(
                self,
                Self::None
                    | Self::AddToBook
                    | Self::PinRepresentativeThumb
                    | Self::ToggleDetachedViewer
                    | Self::ToggleWindowMode
                    | Self::CycleFavorite
                    | Self::GridHistoryBack
                    | Self::GridHistoryForward
                    | Self::TreeFolderPrev
                    | Self::TreeFolderNext
                    | Self::SiblingFolderPrev
                    | Self::SiblingFolderNext
                    | Self::ImageHome
                    | Self::ImageEnd
                    | Self::ImageRotateLeft
                    | Self::ImageRotateRight
                    | Self::ImageCapture
                    | Self::ImageToggleMetadata
                    | Self::ImageSlideshow
                    | Self::ImagePixelGrid
                    | Self::ImageBackgroundCycle
                    | Self::ImageComparePin
                    | Self::ImageCopyToClipboard
                    | Self::ImageOpenFolder
                    | Self::ImageCopyPath
                    | Self::ImageCopyFileName
            ),
            RingShortcutContext::VideoFullscreen => matches!(
                self,
                Self::None
                    | Self::AddToBook
                    | Self::PinRepresentativeThumb
                    | Self::ToggleDetachedViewer
                    | Self::ToggleWindowMode
                    | Self::CycleFavorite
                    | Self::GridHistoryBack
                    | Self::GridHistoryForward
                    | Self::TreeFolderPrev
                    | Self::TreeFolderNext
                    | Self::SiblingFolderPrev
                    | Self::SiblingFolderNext
                    | Self::VideoCapture
                    | Self::VideoMute
                    | Self::VideoLoop
                    | Self::VideoBookmark
                    | Self::VideoMarkerPrev
                    | Self::VideoMarkerNext
                    | Self::VideoTileMode
                    | Self::VideoExternalPlayer
            ),
        }
    }

    pub fn available_for_context(context: RingShortcutContext) -> Vec<Self> {
        let actions = match context {
            RingShortcutContext::Grid => vec![
                Self::None,
                Self::ToggleMaximize,
                Self::AddToBook,
                Self::PinRepresentativeThumb,
                Self::CycleFavorite,
                Self::GridToggleDetails,
                Self::GridToggleSnapshotLock,
                Self::GridToggleCheck,
                Self::GridSelectAll,
                Self::GridHistoryBack,
                Self::GridHistoryForward,
                Self::GridParentFolder,
                Self::TreeFolderPrev,
                Self::TreeFolderNext,
                Self::SiblingFolderPrev,
                Self::SiblingFolderNext,
            ],
            RingShortcutContext::ImageFullscreen => vec![
                Self::None,
                Self::AddToBook,
                Self::PinRepresentativeThumb,
                Self::ToggleWindowMode,
                Self::ToggleDetachedViewer,
                Self::CycleFavorite,
                Self::GridHistoryBack,
                Self::GridHistoryForward,
                Self::TreeFolderPrev,
                Self::TreeFolderNext,
                Self::SiblingFolderPrev,
                Self::SiblingFolderNext,
                Self::ImageHome,
                Self::ImageEnd,
                Self::ImageRotateLeft,
                Self::ImageRotateRight,
                Self::ImageCapture,
                Self::ImageToggleMetadata,
                Self::ImageSlideshow,
                Self::ImagePixelGrid,
                Self::ImageBackgroundCycle,
                Self::ImageComparePin,
                Self::ImageCopyToClipboard,
                Self::ImageOpenFolder,
                Self::ImageCopyPath,
                Self::ImageCopyFileName,
            ],
            RingShortcutContext::VideoFullscreen => vec![
                Self::None,
                Self::AddToBook,
                Self::PinRepresentativeThumb,
                Self::ToggleWindowMode,
                Self::ToggleDetachedViewer,
                Self::CycleFavorite,
                Self::GridHistoryBack,
                Self::GridHistoryForward,
                Self::TreeFolderPrev,
                Self::TreeFolderNext,
                Self::SiblingFolderPrev,
                Self::SiblingFolderNext,
                Self::VideoCapture,
                Self::VideoMute,
                Self::VideoLoop,
                Self::VideoBookmark,
                Self::VideoMarkerPrev,
                Self::VideoMarkerNext,
                Self::VideoTileMode,
                Self::VideoExternalPlayer,
            ],
        };
        actions
    }
}

impl Default for RingActionId {
    fn default() -> Self {
        Self::None
    }
}

impl Serialize for RingActionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RingActionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_str(&value).unwrap_or(Self::Unknown(value)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MouseBackForwardActionId {
    None,
    FolderHistoryPrevNext,
    TreeFolderPrevNext,
    Unknown(String),
}

impl MouseBackForwardActionId {
    pub fn as_str(&self) -> &str {
        match self {
            Self::None => "none",
            Self::FolderHistoryPrevNext => "folder_history_prev_next",
            Self::TreeFolderPrevNext => "tree_folder_prev_next",
            Self::Unknown(s) => s.as_str(),
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "none" | "" => Self::None,
            "folder_history_prev_next" => Self::FolderHistoryPrevNext,
            "tree_folder_prev_next" => Self::TreeFolderPrevNext,
            _ => return None,
        })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "未設定 (従来どおり)",
            Self::FolderHistoryPrevNext => "フォルダ履歴 戻る/進む",
            Self::TreeFolderPrevNext => "ツリー順 前/次フォルダ",
            Self::Unknown(_) => "不明な設定",
        }
    }

    pub fn effective(&self) -> Self {
        match self {
            Self::FolderHistoryPrevNext => Self::FolderHistoryPrevNext,
            Self::TreeFolderPrevNext => Self::TreeFolderPrevNext,
            Self::None | Self::Unknown(_) => Self::TreeFolderPrevNext,
        }
    }
}

impl Default for MouseBackForwardActionId {
    fn default() -> Self {
        Self::None
    }
}

impl Serialize for MouseBackForwardActionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MouseBackForwardActionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_str(&value).unwrap_or(Self::Unknown(value)))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MouseButtonProfile {
    #[serde(default = "default_mouse_back_button_action")]
    pub back: RingActionId,
    #[serde(default = "default_mouse_forward_button_action")]
    pub forward: RingActionId,
}

impl MouseButtonProfile {
    pub fn new(back: RingActionId, forward: RingActionId) -> Self {
        Self { back, forward }
    }

    pub fn action(&self, forward: bool) -> RingActionId {
        if forward {
            self.forward.clone()
        } else {
            self.back.clone()
        }
    }

    pub fn sanitize(&mut self, context: RingShortcutContext) {
        if !self.back.is_valid_for_context(context) {
            self.back = RingActionId::None;
        }
        if !self.forward.is_valid_for_context(context) {
            self.forward = RingActionId::None;
        }
    }
}

impl Default for MouseButtonProfile {
    fn default() -> Self {
        default_mouse_button_profile()
    }
}

fn default_mouse_back_button_action() -> RingActionId {
    RingActionId::GridHistoryBack
}

fn default_mouse_forward_button_action() -> RingActionId {
    RingActionId::GridHistoryForward
}

fn default_mouse_button_profile() -> MouseButtonProfile {
    MouseButtonProfile::new(
        default_mouse_back_button_action(),
        default_mouse_forward_button_action(),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WheelPairActionId {
    None,
    FolderHistoryPrevNext,
    TreeFolderPrevNext,
    SiblingFolderPrevNext,
    PageJumpPrevNext,
    ZoomInOut,
    VideoVolumeUpDown,
    VideoMarkerPrevNext,
    Unknown(String),
}

impl WheelPairActionId {
    pub fn as_str(&self) -> &str {
        match self {
            Self::None => "none",
            Self::FolderHistoryPrevNext => "folder_history_prev_next",
            Self::TreeFolderPrevNext => "tree_folder_prev_next",
            Self::SiblingFolderPrevNext => "sibling_folder_prev_next",
            Self::PageJumpPrevNext => "page_jump_prev_next",
            Self::ZoomInOut => "zoom_in_out",
            Self::VideoVolumeUpDown => "video_volume_up_down",
            Self::VideoMarkerPrevNext => "video_marker_prev_next",
            Self::Unknown(s) => s.as_str(),
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "none" | "" => Self::None,
            "folder_history_prev_next" => Self::FolderHistoryPrevNext,
            "tree_folder_prev_next" => Self::TreeFolderPrevNext,
            "sibling_folder_prev_next" => Self::SiblingFolderPrevNext,
            "page_jump_prev_next" => Self::PageJumpPrevNext,
            "zoom_in_out" => Self::ZoomInOut,
            "video_volume_up_down" => Self::VideoVolumeUpDown,
            "video_marker_prev_next" => Self::VideoMarkerPrevNext,
            _ => return None,
        })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "なし",
            Self::FolderHistoryPrevNext => "フォルダ履歴 戻る/進む",
            Self::TreeFolderPrevNext => "ツリー順 前/次フォルダ",
            Self::SiblingFolderPrevNext => "兄弟フォルダ 前/次",
            Self::PageJumpPrevNext => "ページジャンプ 前/次",
            Self::ZoomInOut => "ズーム イン/アウト",
            Self::VideoVolumeUpDown => "動画音量 上げる/下げる",
            Self::VideoMarkerPrevNext => "動画マーカー 前/次",
            Self::Unknown(_) => "不明な設定",
        }
    }

    pub fn available() -> &'static [Self] {
        const AVAILABLE: &[WheelPairActionId] = &[
            WheelPairActionId::None,
            WheelPairActionId::FolderHistoryPrevNext,
            WheelPairActionId::TreeFolderPrevNext,
            WheelPairActionId::SiblingFolderPrevNext,
            WheelPairActionId::PageJumpPrevNext,
            WheelPairActionId::ZoomInOut,
            WheelPairActionId::VideoVolumeUpDown,
            WheelPairActionId::VideoMarkerPrevNext,
        ];
        AVAILABLE
    }

    pub fn is_valid_for_context(&self, context: RingShortcutContext) -> bool {
        match self {
            Self::None | Self::Unknown(_) => false,
            Self::FolderHistoryPrevNext
            | Self::TreeFolderPrevNext
            | Self::SiblingFolderPrevNext => true,
            Self::PageJumpPrevNext | Self::ZoomInOut => {
                context == RingShortcutContext::ImageFullscreen
            }
            Self::VideoVolumeUpDown | Self::VideoMarkerPrevNext => {
                context == RingShortcutContext::VideoFullscreen
            }
        }
    }
}

impl Default for WheelPairActionId {
    fn default() -> Self {
        Self::None
    }
}

impl Serialize for WheelPairActionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WheelPairActionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_str(&value).unwrap_or(Self::Unknown(value)))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RingShortcutProfile {
    #[serde(default = "default_ring_slots")]
    pub slots: Vec<RingActionId>,
}

impl RingShortcutProfile {
    pub fn new(slots: Vec<RingActionId>) -> Self {
        let mut profile = Self { slots };
        profile.ensure_slot_count();
        profile
    }

    pub fn sanitize(&mut self, context: RingShortcutContext) {
        self.ensure_slot_count();
        for action in &mut self.slots {
            if !action.is_valid_for_context(context) {
                *action = RingActionId::None;
            }
        }
    }

    fn ensure_slot_count(&mut self) {
        self.slots
            .resize(RING_SHORTCUT_SLOT_COUNT, RingActionId::None);
        self.slots.truncate(RING_SHORTCUT_SLOT_COUNT);
    }
}

impl Default for RingShortcutProfile {
    fn default() -> Self {
        Self {
            slots: default_ring_slots(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RingShortcutSettings {
    // Compatibility only: old builds used this as a global right-drag ring toggle.
    // Missing per-context modes inherit this value; new UI writes right_drag_*.
    #[serde(default)]
    pub mouse_flick_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_drag_grid: Option<RightDragMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_drag_image: Option<RightDragMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_drag_video: Option<RightDragMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_drag_edit: Option<RightDragMode>,
    #[serde(default = "default_grid_gesture_profile")]
    pub mouse_gestures_grid: MouseGestureProfile,
    #[serde(default = "default_image_gesture_profile")]
    pub mouse_gestures_image: MouseGestureProfile,
    #[serde(default = "default_video_gesture_profile")]
    pub mouse_gestures_video: MouseGestureProfile,
    #[serde(default = "default_edit_gesture_profile")]
    pub mouse_gestures_edit: MouseGestureProfile,
    // Compatibility only: X ring/picker is always enabled. Old saved `false`
    // values are normalized during Settings::load()/sanitize().
    #[serde(default = "default_true")]
    pub gamepad_ring_enabled: bool,
    // Compatibility only: Shift/Alt wheel customization is postponed.
    #[serde(default)]
    pub shift_wheel_pair: WheelPairActionId,
    #[serde(default)]
    pub alt_wheel_pair: WheelPairActionId,
    #[serde(default)]
    pub mouse_back_forward_action: MouseBackForwardActionId,
    #[serde(default = "default_mouse_button_profile")]
    pub mouse_buttons_grid: MouseButtonProfile,
    #[serde(default = "default_mouse_button_profile")]
    pub mouse_buttons_image: MouseButtonProfile,
    #[serde(default = "default_mouse_button_profile")]
    pub mouse_buttons_video: MouseButtonProfile,
    #[serde(default)]
    pub mouse_nav_prompt_done: bool,
    #[serde(default)]
    pub x_picker_hint_shown: bool,
    #[serde(default = "default_grid_profile")]
    pub grid: RingShortcutProfile,
    #[serde(default = "default_image_profile")]
    pub image: RingShortcutProfile,
    #[serde(default = "default_video_profile")]
    pub video: RingShortcutProfile,
}

impl RingShortcutSettings {
    pub fn right_drag_mode(&self, context: RightDragContext) -> RightDragMode {
        let configured = match context {
            RightDragContext::Grid => &self.right_drag_grid,
            RightDragContext::ImageFullscreen => &self.right_drag_image,
            RightDragContext::VideoFullscreen => &self.right_drag_video,
            RightDragContext::EditMode => &self.right_drag_edit,
        };
        if let Some(mode) = configured {
            return mode.effective();
        }
        if self.mouse_flick_enabled && context.ring_context().is_some() {
            RightDragMode::RingShortcut
        } else {
            RightDragMode::Disabled
        }
    }

    pub fn set_right_drag_mode(&mut self, context: RightDragContext, mode: RightDragMode) {
        let target = match context {
            RightDragContext::Grid => &mut self.right_drag_grid,
            RightDragContext::ImageFullscreen => &mut self.right_drag_image,
            RightDragContext::VideoFullscreen => &mut self.right_drag_video,
            RightDragContext::EditMode => &mut self.right_drag_edit,
        };
        *target = Some(mode.effective());
    }

    pub fn mouse_gesture_profile(&self, context: RightDragContext) -> &MouseGestureProfile {
        match context {
            RightDragContext::Grid => &self.mouse_gestures_grid,
            RightDragContext::ImageFullscreen => &self.mouse_gestures_image,
            RightDragContext::VideoFullscreen => &self.mouse_gestures_video,
            RightDragContext::EditMode => &self.mouse_gestures_edit,
        }
    }

    pub fn mouse_gesture_profile_mut(
        &mut self,
        context: RightDragContext,
    ) -> &mut MouseGestureProfile {
        match context {
            RightDragContext::Grid => &mut self.mouse_gestures_grid,
            RightDragContext::ImageFullscreen => &mut self.mouse_gestures_image,
            RightDragContext::VideoFullscreen => &mut self.mouse_gestures_video,
            RightDragContext::EditMode => &mut self.mouse_gestures_edit,
        }
    }

    pub fn reset_mouse_gesture_profile(&mut self, context: RightDragContext) {
        *self.mouse_gesture_profile_mut(context) = default_gesture_profile_for_context(context);
    }
    pub fn mouse_ring_enabled(&self, context: RingShortcutContext) -> bool {
        let right_drag_context = match context {
            RingShortcutContext::Grid => RightDragContext::Grid,
            RingShortcutContext::ImageFullscreen => RightDragContext::ImageFullscreen,
            RingShortcutContext::VideoFullscreen => RightDragContext::VideoFullscreen,
        };
        self.right_drag_mode(right_drag_context) == RightDragMode::RingShortcut
    }

    fn sanitize_right_drag_mode(mode: &mut Option<RightDragMode>) {
        if matches!(mode, Some(RightDragMode::Unknown(_))) {
            *mode = Some(RightDragMode::Disabled);
        }
    }

    pub fn profile(&self, context: RingShortcutContext) -> &RingShortcutProfile {
        match context {
            RingShortcutContext::Grid => &self.grid,
            RingShortcutContext::ImageFullscreen => &self.image,
            RingShortcutContext::VideoFullscreen => &self.video,
        }
    }

    pub fn profile_mut(&mut self, context: RingShortcutContext) -> &mut RingShortcutProfile {
        match context {
            RingShortcutContext::Grid => &mut self.grid,
            RingShortcutContext::ImageFullscreen => &mut self.image,
            RingShortcutContext::VideoFullscreen => &mut self.video,
        }
    }

    pub fn reset_profile(&mut self, context: RingShortcutContext) {
        *self.profile_mut(context) = default_profile_for_context(context);
    }

    pub fn mouse_button_profile(&self, context: RingShortcutContext) -> &MouseButtonProfile {
        match context {
            RingShortcutContext::Grid => &self.mouse_buttons_grid,
            RingShortcutContext::ImageFullscreen => &self.mouse_buttons_image,
            RingShortcutContext::VideoFullscreen => &self.mouse_buttons_video,
        }
    }

    pub fn mouse_button_profile_mut(
        &mut self,
        context: RingShortcutContext,
    ) -> &mut MouseButtonProfile {
        match context {
            RingShortcutContext::Grid => &mut self.mouse_buttons_grid,
            RingShortcutContext::ImageFullscreen => &mut self.mouse_buttons_image,
            RingShortcutContext::VideoFullscreen => &mut self.mouse_buttons_video,
        }
    }

    pub fn reset_mouse_button_profile(&mut self, context: RingShortcutContext) {
        *self.mouse_button_profile_mut(context) = default_mouse_button_profile();
    }

    pub fn set_mouse_buttons_from_legacy_pair(&mut self, action: MouseBackForwardActionId) {
        let Some((back, forward)) = legacy_mouse_button_pair(action) else {
            return;
        };
        let profile = MouseButtonProfile::new(back, forward);
        self.mouse_buttons_grid = profile.clone();
        self.mouse_buttons_image = profile.clone();
        self.mouse_buttons_video = profile;
    }

    pub fn sanitize(&mut self) {
        self.gamepad_ring_enabled = true;
        Self::sanitize_right_drag_mode(&mut self.right_drag_grid);
        Self::sanitize_right_drag_mode(&mut self.right_drag_image);
        Self::sanitize_right_drag_mode(&mut self.right_drag_video);
        Self::sanitize_right_drag_mode(&mut self.right_drag_edit);
        self.mouse_gestures_grid.sanitize(RightDragContext::Grid);
        self.mouse_gestures_image
            .sanitize(RightDragContext::ImageFullscreen);
        self.mouse_gestures_video
            .sanitize(RightDragContext::VideoFullscreen);
        self.mouse_gestures_edit
            .sanitize(RightDragContext::EditMode);
        self.grid.sanitize(RingShortcutContext::Grid);
        self.image.sanitize(RingShortcutContext::ImageFullscreen);
        self.video.sanitize(RingShortcutContext::VideoFullscreen);
        self.mouse_buttons_grid.sanitize(RingShortcutContext::Grid);
        self.mouse_buttons_image
            .sanitize(RingShortcutContext::ImageFullscreen);
        self.mouse_buttons_video
            .sanitize(RingShortcutContext::VideoFullscreen);
        if matches!(self.shift_wheel_pair, WheelPairActionId::Unknown(_)) {
            self.shift_wheel_pair = WheelPairActionId::None;
        }
        if matches!(self.alt_wheel_pair, WheelPairActionId::Unknown(_)) {
            self.alt_wheel_pair = WheelPairActionId::None;
        }
        match self.mouse_back_forward_action.clone() {
            MouseBackForwardActionId::FolderHistoryPrevNext
            | MouseBackForwardActionId::TreeFolderPrevNext => {
                self.set_mouse_buttons_from_legacy_pair(self.mouse_back_forward_action.clone());
                self.mouse_back_forward_action = MouseBackForwardActionId::None;
                self.mouse_nav_prompt_done = true;
            }
            MouseBackForwardActionId::Unknown(_) => {
                self.mouse_back_forward_action = MouseBackForwardActionId::None;
            }
            MouseBackForwardActionId::None => {}
        }
    }
}

impl Default for RingShortcutSettings {
    fn default() -> Self {
        Self {
            mouse_flick_enabled: false,
            right_drag_grid: None,
            right_drag_image: None,
            right_drag_video: None,
            right_drag_edit: None,
            mouse_gestures_grid: default_grid_gesture_profile(),
            mouse_gestures_image: default_image_gesture_profile(),
            mouse_gestures_video: default_video_gesture_profile(),
            mouse_gestures_edit: default_edit_gesture_profile(),
            gamepad_ring_enabled: true,
            shift_wheel_pair: WheelPairActionId::None,
            alt_wheel_pair: WheelPairActionId::None,
            mouse_back_forward_action: MouseBackForwardActionId::None,
            mouse_buttons_grid: default_mouse_button_profile(),
            mouse_buttons_image: default_mouse_button_profile(),
            mouse_buttons_video: default_mouse_button_profile(),
            mouse_nav_prompt_done: false,
            x_picker_hint_shown: false,
            grid: default_grid_profile(),
            image: default_image_profile(),
            video: default_video_profile(),
        }
    }
}

fn default_true() -> bool {
    true
}

pub fn legacy_mouse_button_pair(
    action: MouseBackForwardActionId,
) -> Option<(RingActionId, RingActionId)> {
    match action {
        MouseBackForwardActionId::FolderHistoryPrevNext => Some((
            RingActionId::GridHistoryBack,
            RingActionId::GridHistoryForward,
        )),
        MouseBackForwardActionId::TreeFolderPrevNext => {
            Some((RingActionId::TreeFolderPrev, RingActionId::TreeFolderNext))
        }
        MouseBackForwardActionId::None | MouseBackForwardActionId::Unknown(_) => None,
    }
}

fn default_ring_slots() -> Vec<RingActionId> {
    vec![RingActionId::None; RING_SHORTCUT_SLOT_COUNT]
}

pub fn default_profile_for_context(context: RingShortcutContext) -> RingShortcutProfile {
    match context {
        RingShortcutContext::Grid => default_grid_profile(),
        RingShortcutContext::ImageFullscreen => default_image_profile(),
        RingShortcutContext::VideoFullscreen => default_video_profile(),
    }
}

pub fn default_gesture_profile_for_context(context: RightDragContext) -> MouseGestureProfile {
    match context {
        RightDragContext::Grid => default_grid_gesture_profile(),
        RightDragContext::ImageFullscreen => default_image_gesture_profile(),
        RightDragContext::VideoFullscreen => default_video_gesture_profile(),
        RightDragContext::EditMode => default_edit_gesture_profile(),
    }
}

fn gesture_binding(pattern: &[MouseGestureDirection], action: RingActionId) -> MouseGestureBinding {
    MouseGestureBinding::new(pattern.to_vec(), action)
}

fn default_grid_gesture_profile() -> MouseGestureProfile {
    use MouseGestureDirection::*;
    MouseGestureProfile {
        bindings: vec![
            gesture_binding(&[Up], RingActionId::GridParentFolder),
            gesture_binding(&[Right], RingActionId::GridHistoryForward),
            gesture_binding(&[Down], RingActionId::PinRepresentativeThumb),
            gesture_binding(&[Left], RingActionId::GridHistoryBack),
        ],
    }
}

fn default_image_gesture_profile() -> MouseGestureProfile {
    use MouseGestureDirection::*;
    MouseGestureProfile {
        bindings: vec![
            gesture_binding(&[Up], RingActionId::ImageSlideshow),
            gesture_binding(&[Right], RingActionId::ImageRotateRight),
            gesture_binding(&[Down], RingActionId::PinRepresentativeThumb),
            gesture_binding(&[Left], RingActionId::ImageRotateLeft),
        ],
    }
}

fn default_video_gesture_profile() -> MouseGestureProfile {
    use MouseGestureDirection::*;
    MouseGestureProfile {
        bindings: vec![
            gesture_binding(&[Up], RingActionId::VideoLoop),
            gesture_binding(&[Right], RingActionId::VideoTileMode),
            gesture_binding(&[Down], RingActionId::PinRepresentativeThumb),
            gesture_binding(&[Left], RingActionId::VideoMute),
        ],
    }
}

fn default_edit_gesture_profile() -> MouseGestureProfile {
    MouseGestureProfile {
        bindings: Vec::new(),
    }
}
fn default_grid_profile() -> RingShortcutProfile {
    RingShortcutProfile::new(vec![
        RingActionId::GridParentFolder,
        RingActionId::GridToggleSnapshotLock,
        RingActionId::GridHistoryForward,
        RingActionId::GridToggleCheck,
        RingActionId::PinRepresentativeThumb,
        RingActionId::AddToBook,
        RingActionId::GridHistoryBack,
        RingActionId::GridToggleDetails,
    ])
}

fn default_image_profile() -> RingShortcutProfile {
    RingShortcutProfile::new(vec![
        RingActionId::ImageSlideshow,
        RingActionId::ImageToggleMetadata,
        RingActionId::ImageRotateRight,
        RingActionId::None,
        RingActionId::PinRepresentativeThumb,
        RingActionId::AddToBook,
        RingActionId::ImageRotateLeft,
        RingActionId::ImageCapture,
    ])
}

fn default_video_profile() -> RingShortcutProfile {
    RingShortcutProfile::new(vec![
        RingActionId::VideoLoop,
        RingActionId::VideoExternalPlayer,
        RingActionId::VideoTileMode,
        RingActionId::VideoBookmark,
        RingActionId::PinRepresentativeThumb,
        RingActionId::AddToBook,
        RingActionId::VideoMute,
        RingActionId::VideoCapture,
    ])
}

#[derive(Clone, Debug, PartialEq)]
pub enum PickerCommand {
    SetGridColumns(usize),
    SetSortOrder(crate::settings::SortOrder),
    SetThumbAspectAuto,
    SetThumbAspect(crate::settings::ThumbAspect),
    SetItemRating(u8),
    SetContainerRating(u8),
    SetSpreadMode(crate::settings::SpreadMode),
    SetReadingFlow(crate::settings::ReadingFlow),
    SetReadingDirection(crate::settings::ReadingDirection),
    SetFitMode(crate::settings::FullscreenFitMode),
    SetPostFilter(crate::adjustment::PostFilter),
    SetUpscaleModel(crate::ai::ModelKind),
    SetVideoVolume(f64),
    SetVideoPlaybackSpeed(f64),
    SetVideoContinuousMode(crate::video::VideoContinuousMode),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingPickerRowId {
    GridColumns,
    GridSortOrder,
    GridThumbAspect,
    ItemRating,
    ContainerRating,
    SpreadMode,
    ReadingFlow,
    ReadingDirection,
    FitMode,
    PostFilter,
    UpscaleModel,
    VideoVolume,
    VideoPlaybackSpeed,
    VideoContinuousMode,
}

impl RingPickerRowId {
    pub fn label(self) -> &'static str {
        match self {
            Self::GridColumns => "列数",
            Self::GridSortOrder => "ソート",
            Self::GridThumbAspect => "サムネ比率",
            Self::ItemRating => "アイテム評価",
            Self::ContainerRating => "コンテナ評価",
            Self::SpreadMode => "見開き",
            Self::ReadingFlow => "連結方式",
            Self::ReadingDirection => "読み方向",
            Self::FitMode => "フィット",
            Self::PostFilter => "ポストフィルタ",
            Self::UpscaleModel => "アップスケール",
            Self::VideoVolume => "音量",
            Self::VideoPlaybackSpeed => "再生速度",
            Self::VideoContinuousMode => "連続再生",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RingPickerOriginalState {
    pub grid_cols: usize,
    pub sort_order: crate::settings::SortOrder,
    pub thumb_aspect_auto: bool,
    pub thumb_aspect: crate::settings::ThumbAspect,
    pub item_rating_records: Vec<(usize, u8)>,
    pub container_rating: u8,
    pub spread_mode: crate::settings::SpreadMode,
    pub reading_flow: crate::settings::ReadingFlow,
    pub reading_direction: crate::settings::ReadingDirection,
    pub fit_mode: crate::settings::FullscreenFitMode,
    pub post_filter: crate::adjustment::PostFilter,
    pub upscale_model_key: Option<String>,
    pub video_volume: f64,
    pub video_playback_speed: f64,
    pub video_continuous_mode: crate::video::VideoContinuousMode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RingPickerState {
    pub context: RingShortcutContext,
    pub anchor: RingPickerAnchor,
    pub original: RingPickerOriginalState,
    pub row: usize,
    pub dirty_rows: Vec<RingPickerRowId>,
    pub x_close_armed: bool,
    pub drill: Option<PickerListState>,
    pub grid_cols: usize,
    pub sort_order: crate::settings::SortOrder,
    pub thumb_aspect_auto: bool,
    pub thumb_aspect: crate::settings::ThumbAspect,
    pub item_rating: u8,
    pub container_rating: u8,
    pub spread_mode: crate::settings::SpreadMode,
    pub reading_flow: crate::settings::ReadingFlow,
    pub reading_direction: crate::settings::ReadingDirection,
    pub fit_mode: crate::settings::FullscreenFitMode,
    pub post_filter: crate::adjustment::PostFilter,
    pub upscale_model_key: Option<String>,
    pub video_volume: f64,
    pub video_playback_speed: f64,
    pub video_continuous_mode: crate::video::VideoContinuousMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RingPickerAnchor {
    pub folder: Option<PathBuf>,
    pub item_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GamepadFavoritePickerState {
    pub selected: usize,
    pub scroll_top: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GamepadLocationNav {
    Direct(PathBuf),
    DriveList,
    ReadingHistory,
    RatingView(u8),
    BooksRoot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GamepadLocationEntry {
    pub label: String,
    pub value: String,
    pub nav: GamepadLocationNav,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GamepadLocationPickerState {
    pub selected: usize,
    pub scroll_top: usize,
    pub entries: Vec<GamepadLocationEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GamepadVideoMarkerPickerState {
    pub selected: usize,
    pub scroll_top: usize,
}

impl RingPickerState {
    pub fn current_row(&self) -> usize {
        self.row
    }

    pub fn clamp_row(&mut self, row_count: usize) {
        if row_count == 0 {
            self.row = 0;
        } else if self.row >= row_count {
            self.row = row_count - 1;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerListMode {
    RowValues(RingPickerRowId),
    PostFilterGroup,
    PostFilterItem { group: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickerListState {
    pub mode: PickerListMode,
    pub selected: usize,
}

#[derive(Clone, Debug)]
pub struct MouseGestureState {
    pub context: RightDragContext,
    pub start_time: Instant,
    pub start_pos: egui::Pos2,
    pub last_step_pos: egui::Pos2,
    pub current_pos: egui::Pos2,
    pub pattern: Vec<MouseGestureDirection>,
    pub armed: bool,
}

impl MouseGestureState {
    pub fn new(context: RightDragContext, start_time: Instant, start_pos: egui::Pos2) -> Self {
        Self {
            context,
            start_time,
            start_pos,
            last_step_pos: start_pos,
            current_pos: start_pos,
            pattern: Vec::new(),
            armed: false,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    pub fn moved(&self) -> f32 {
        self.current_pos.distance(self.start_pos)
    }

    pub fn guide_visible(&self) -> bool {
        self.armed || self.elapsed() >= mouse_flick_menu_delay()
    }
}
#[derive(Clone, Debug)]
pub struct MouseFlickState {
    pub context: RingShortcutContext,
    pub start_time: Instant,
    pub start_pos: egui::Pos2,
    pub current_pos: egui::Pos2,
    pub armed: bool,
}

impl MouseFlickState {
    pub fn new(context: RingShortcutContext, start_time: Instant, start_pos: egui::Pos2) -> Self {
        Self {
            context,
            start_time,
            start_pos,
            current_pos: start_pos,
            armed: false,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    pub fn moved(&self) -> f32 {
        self.current_pos.distance(self.start_pos)
    }

    pub fn guide_visible(&self) -> bool {
        self.armed || self.elapsed() >= mouse_flick_menu_delay()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MouseFlickOutcome {
    None,
    ShortTap,
    Cancelled,
    Fired,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profiles_match_design_slots() {
        let defaults = RingShortcutSettings::default();
        assert_eq!(defaults.mouse_flick_enabled, false);
        for &context in RightDragContext::all() {
            assert_eq!(defaults.right_drag_mode(context), RightDragMode::Disabled);
        }
        assert_eq!(
            defaults
                .mouse_gestures_grid
                .action_for_pattern(&[MouseGestureDirection::Up]),
            RingActionId::GridParentFolder
        );
        assert_eq!(
            defaults
                .mouse_gestures_image
                .action_for_pattern(&[MouseGestureDirection::Right]),
            RingActionId::ImageRotateRight
        );
        assert_eq!(
            defaults
                .mouse_gestures_video
                .action_for_pattern(&[MouseGestureDirection::Left]),
            RingActionId::VideoMute
        );
        assert!(defaults.mouse_gestures_edit.bindings.is_empty());
        assert_eq!(defaults.gamepad_ring_enabled, true);
        assert_eq!(defaults.shift_wheel_pair, WheelPairActionId::None);
        assert_eq!(defaults.alt_wheel_pair, WheelPairActionId::None);
        assert_eq!(
            defaults.mouse_back_forward_action,
            MouseBackForwardActionId::None
        );
        for &context in RingShortcutContext::all() {
            let profile = defaults.mouse_button_profile(context);
            assert_eq!(profile.back, RingActionId::GridHistoryBack);
            assert_eq!(profile.forward, RingActionId::GridHistoryForward);
        }
        assert_eq!(defaults.mouse_nav_prompt_done, false);
        assert_eq!(defaults.x_picker_hint_shown, false);
        assert_eq!(
            defaults.grid.slots[RingDirection::Up.slot_index()],
            RingActionId::GridParentFolder
        );
        assert_eq!(
            defaults.grid.slots[RingDirection::DownLeft.slot_index()],
            RingActionId::AddToBook
        );
        assert_eq!(
            defaults.grid.slots[RingDirection::DownRight.slot_index()],
            RingActionId::GridToggleCheck
        );
        assert_eq!(
            defaults.image.slots[RingDirection::Up.slot_index()],
            RingActionId::ImageSlideshow
        );
        assert_eq!(
            defaults.image.slots[RingDirection::UpRight.slot_index()],
            RingActionId::ImageToggleMetadata
        );
        assert_eq!(
            defaults.image.slots[RingDirection::DownLeft.slot_index()],
            RingActionId::AddToBook
        );
        assert_eq!(
            defaults.image.slots[RingDirection::UpLeft.slot_index()],
            RingActionId::ImageCapture
        );
        assert_eq!(
            defaults.video.slots[RingDirection::Up.slot_index()],
            RingActionId::VideoLoop
        );
        assert_eq!(
            defaults.video.slots[RingDirection::DownLeft.slot_index()],
            RingActionId::AddToBook
        );
        assert_eq!(
            defaults.video.slots[RingDirection::DownRight.slot_index()],
            RingActionId::VideoBookmark
        );
        assert_eq!(
            defaults.video.slots[RingDirection::UpLeft.slot_index()],
            RingActionId::VideoCapture
        );
    }

    #[test]
    fn mouse_gesture_patterns_parse_symbols_and_letters() {
        use MouseGestureDirection::*;
        assert_eq!(parse_mouse_gesture_pattern("↓→"), Some(vec![Down, Right]));
        assert_eq!(parse_mouse_gesture_pattern("D R"), Some(vec![Down, Right]));
        assert_eq!(parse_mouse_gesture_pattern("up"), Some(vec![Up]));
        assert_eq!(
            parse_mouse_gesture_pattern("down,right"),
            Some(vec![Down, Right])
        );
        assert_eq!(parse_mouse_gesture_pattern("↓↓→"), Some(vec![Down, Right]));
        assert_eq!(
            parse_mouse_gesture_pattern("URDL"),
            Some(vec![Up, Right, Down, Left])
        );
        assert_eq!(parse_mouse_gesture_pattern("URDLU"), None);
        assert_eq!(format_mouse_gesture_pattern(&[Down, Right]), "↓→");
    }
    #[test]
    fn mouse_gesture_profile_ignores_unknown_directions_on_load() {
        let mut profile: MouseGestureProfile = serde_json::from_str(
            r#"{"bindings":[{"pattern":["up","future","right"],"action":"grid_parent_folder"}]}"#,
        )
        .unwrap();

        profile.sanitize(RightDragContext::Grid);

        assert_eq!(profile.bindings.len(), 1);
        assert_eq!(
            profile.bindings[0].pattern,
            vec![MouseGestureDirection::Up, MouseGestureDirection::Right]
        );
        assert_eq!(profile.bindings[0].action, RingActionId::GridParentFolder);
    }
    #[test]
    fn right_drag_modes_inherit_legacy_toggle_until_configured() {
        let mut settings = RingShortcutSettings::default();
        settings.mouse_flick_enabled = true;
        assert_eq!(
            settings.right_drag_mode(RightDragContext::Grid),
            RightDragMode::RingShortcut
        );
        assert_eq!(
            settings.right_drag_mode(RightDragContext::ImageFullscreen),
            RightDragMode::RingShortcut
        );
        assert_eq!(
            settings.right_drag_mode(RightDragContext::VideoFullscreen),
            RightDragMode::RingShortcut
        );
        assert_eq!(
            settings.right_drag_mode(RightDragContext::EditMode),
            RightDragMode::Disabled
        );

        settings.set_right_drag_mode(RightDragContext::Grid, RightDragMode::Disabled);
        settings.set_right_drag_mode(
            RightDragContext::ImageFullscreen,
            RightDragMode::MouseGesture,
        );
        assert_eq!(
            settings.right_drag_mode(RightDragContext::Grid),
            RightDragMode::Disabled
        );
        assert_eq!(
            settings.right_drag_mode(RightDragContext::ImageFullscreen),
            RightDragMode::MouseGesture
        );
        assert_eq!(
            settings.right_drag_mode(RightDragContext::VideoFullscreen),
            RightDragMode::RingShortcut
        );
    }
    #[test]
    fn toggle_maximize_action_round_trips_and_is_grid_only() {
        // as_str <-> from_str (設定永続化のラウンドトリップ)。
        assert_eq!(RingActionId::ToggleMaximize.as_str(), "toggle_maximize");
        assert_eq!(
            RingActionId::from_str("toggle_maximize"),
            Some(RingActionId::ToggleMaximize)
        );
        // グリッド (メインウィンドウ) のリング / マウスボタン両方の候補に出る。
        assert!(RingActionId::ToggleMaximize.is_valid_for_context(RingShortcutContext::Grid));
        assert!(
            RingActionId::available_for_context(RingShortcutContext::Grid)
                .contains(&RingActionId::ToggleMaximize)
        );
        // フルスクリーン (画像/動画) では最大化が無意味なので候補に出さない。
        assert!(
            !RingActionId::ToggleMaximize
                .is_valid_for_context(RingShortcutContext::ImageFullscreen)
        );
        assert!(
            !RingActionId::ToggleMaximize
                .is_valid_for_context(RingShortcutContext::VideoFullscreen)
        );
        assert_eq!(
            RingActionId::ToggleMaximize.label_for_context(RingShortcutContext::Grid),
            "ウィンドウ最大化/復元"
        );
    }

    #[test]
    fn mouse_flick_static_guide_waits_until_long_press() {
        let pos = egui::pos2(10.0, 20.0);
        let flick = MouseFlickState::new(
            RingShortcutContext::ImageFullscreen,
            Instant::now() - mouse_flick_guide_delay() - Duration::from_millis(1),
            pos,
        );
        assert!(!flick.guide_visible());

        let flick = MouseFlickState::new(
            RingShortcutContext::ImageFullscreen,
            Instant::now() - mouse_flick_menu_delay() - Duration::from_millis(1),
            pos,
        );
        assert!(flick.guide_visible());

        let mut flick =
            MouseFlickState::new(RingShortcutContext::ImageFullscreen, Instant::now(), pos);
        flick.armed = true;
        assert!(flick.guide_visible());
    }

    #[test]
    fn sanitize_clears_unknown_and_wrong_context_actions() {
        let mut settings = RingShortcutSettings::default();
        settings.grid.slots[0] = RingActionId::Unknown("future_action".to_string());
        settings.grid.slots[1] = RingActionId::ToggleWindowMode;
        settings.grid.slots[2] = RingActionId::ToggleDetachedViewer;
        settings.image.slots[1] = RingActionId::VideoCapture;
        settings.video.slots.push(RingActionId::ImageCapture);
        settings.shift_wheel_pair = WheelPairActionId::Unknown("future_wheel".to_string());
        settings.mouse_back_forward_action =
            MouseBackForwardActionId::Unknown("future_mouse_nav".to_string());
        settings.mouse_buttons_grid.back = RingActionId::Unknown("future_mouse_button".to_string());
        settings.mouse_buttons_image.forward = RingActionId::VideoCapture;
        settings.mouse_buttons_video.back = RingActionId::ImageCapture;
        settings.right_drag_grid = Some(RightDragMode::Unknown("future_mode".to_string()));
        settings.right_drag_image = Some(RightDragMode::MouseGesture);
        settings
            .mouse_gestures_grid
            .bindings
            .push(MouseGestureBinding::new(
                vec![
                    MouseGestureDirection::Up,
                    MouseGestureDirection::Up,
                    MouseGestureDirection::Right,
                ],
                RingActionId::VideoCapture,
            ));

        settings.sanitize();

        assert_eq!(settings.grid.slots[0], RingActionId::None);
        assert_eq!(settings.grid.slots[1], RingActionId::None);
        assert_eq!(settings.grid.slots[2], RingActionId::None);
        assert_eq!(settings.image.slots[1], RingActionId::None);
        assert_eq!(settings.video.slots.len(), RING_SHORTCUT_SLOT_COUNT);
        assert_eq!(settings.shift_wheel_pair, WheelPairActionId::None);
        assert_eq!(
            settings.mouse_back_forward_action,
            MouseBackForwardActionId::None
        );
        assert_eq!(settings.mouse_buttons_grid.back, RingActionId::None);
        assert_eq!(settings.mouse_buttons_image.forward, RingActionId::None);
        assert_eq!(settings.mouse_buttons_video.back, RingActionId::None);
        assert_eq!(
            settings.right_drag_mode(RightDragContext::Grid),
            RightDragMode::Disabled
        );
        assert_eq!(
            settings.right_drag_mode(RightDragContext::ImageFullscreen),
            RightDragMode::MouseGesture
        );
        let last = settings.mouse_gestures_grid.bindings.last().unwrap();
        assert_eq!(
            last.pattern,
            vec![MouseGestureDirection::Up, MouseGestureDirection::Right]
        );
        assert_eq!(last.action, RingActionId::None);
    }

    #[test]
    fn fullscreen_viewer_toggles_are_available_only_in_fullscreen_contexts() {
        assert!(!RingActionId::ToggleWindowMode.is_valid_for_context(RingShortcutContext::Grid));
        assert!(
            !RingActionId::ToggleDetachedViewer.is_valid_for_context(RingShortcutContext::Grid)
        );
        assert!(
            RingActionId::ToggleWindowMode
                .is_valid_for_context(RingShortcutContext::ImageFullscreen)
        );
        assert!(
            RingActionId::ToggleDetachedViewer
                .is_valid_for_context(RingShortcutContext::ImageFullscreen)
        );
        assert!(
            RingActionId::ToggleWindowMode
                .is_valid_for_context(RingShortcutContext::VideoFullscreen)
        );
        assert!(
            RingActionId::ToggleDetachedViewer
                .is_valid_for_context(RingShortcutContext::VideoFullscreen)
        );

        assert!(
            !RingActionId::available_for_context(RingShortcutContext::Grid)
                .contains(&RingActionId::ToggleWindowMode)
        );
        assert!(
            !RingActionId::available_for_context(RingShortcutContext::Grid)
                .contains(&RingActionId::ToggleDetachedViewer)
        );
        assert!(
            RingActionId::available_for_context(RingShortcutContext::ImageFullscreen)
                .contains(&RingActionId::ToggleWindowMode)
        );
        assert!(
            RingActionId::available_for_context(RingShortcutContext::VideoFullscreen)
                .contains(&RingActionId::ToggleWindowMode)
        );
    }

    #[test]
    fn image_boundary_actions_are_available_only_for_image_fullscreen() {
        for action in [RingActionId::ImageHome, RingActionId::ImageEnd] {
            assert!(!action.is_valid_for_context(RingShortcutContext::Grid));
            assert!(action.is_valid_for_context(RingShortcutContext::ImageFullscreen));
            assert!(!action.is_valid_for_context(RingShortcutContext::VideoFullscreen));
            assert!(
                !RingActionId::available_for_context(RingShortcutContext::Grid).contains(&action)
            );
            assert!(
                RingActionId::available_for_context(RingShortcutContext::ImageFullscreen)
                    .contains(&action)
            );
            assert!(
                !RingActionId::available_for_context(RingShortcutContext::VideoFullscreen)
                    .contains(&action)
            );
        }
    }

    #[test]
    fn sanitize_migrates_legacy_mouse_button_pair_to_profiles() {
        let mut settings = RingShortcutSettings::default();
        settings.mouse_back_forward_action = MouseBackForwardActionId::TreeFolderPrevNext;
        settings.mouse_nav_prompt_done = false;

        settings.sanitize();

        assert_eq!(
            settings.mouse_back_forward_action,
            MouseBackForwardActionId::None
        );
        assert!(settings.mouse_nav_prompt_done);
        for &context in RingShortcutContext::all() {
            let profile = settings.mouse_button_profile(context);
            assert_eq!(profile.back, RingActionId::TreeFolderPrev);
            assert_eq!(profile.forward, RingActionId::TreeFolderNext);
        }
    }

    #[test]
    fn unknown_action_id_deserializes_for_later_sanitize() {
        let action: RingActionId = serde_json::from_str(r#""future_action""#).unwrap();
        assert_eq!(action, RingActionId::Unknown("future_action".to_string()));
        let wheel: WheelPairActionId = serde_json::from_str(r#""future_wheel""#).unwrap();
        assert_eq!(
            wheel,
            WheelPairActionId::Unknown("future_wheel".to_string())
        );
        let mouse_nav: MouseBackForwardActionId =
            serde_json::from_str(r#""future_mouse_nav""#).unwrap();
        assert_eq!(
            mouse_nav,
            MouseBackForwardActionId::Unknown("future_mouse_nav".to_string())
        );
    }
}
