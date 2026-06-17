//! Ring shortcut settings and action identifiers.
//!
//! The input state machines and command execution live in later phases. This
//! module keeps the persisted action ids, context filtering, and defaults in one
//! place so preferences UI and future input dispatch use the same inventory.

use std::time::{Duration, Instant};

use eframe::egui;
use serde::{Deserialize, Serialize};

pub const RING_SHORTCUT_SLOT_COUNT: usize = 8;
pub const MOUSE_FLICK_MOVE_THRESHOLD_PX: f32 = 20.0;
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
    CycleFavorite,
    GridToggleDetails,
    GridToggleSnapshotLock,
    GridToggleCheck,
    GridSelectAll,
    GridHistoryBack,
    GridHistoryForward,
    GridParentFolder,
    ImageRotateLeft,
    ImageRotateRight,
    ImageCapture,
    ImageToggleMetadata,
    ImageSlideshow,
    ImagePixelGrid,
    ImageBackgroundCycle,
    ImageComparePin,
    VideoCapture,
    VideoMute,
    VideoLoop,
    VideoBookmark,
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
            Self::CycleFavorite => "cycle_favorite",
            Self::GridToggleDetails => "grid_toggle_details",
            Self::GridToggleSnapshotLock => "grid_toggle_snapshot_lock",
            Self::GridToggleCheck => "grid_toggle_check",
            Self::GridSelectAll => "grid_select_all",
            Self::GridHistoryBack => "grid_history_back",
            Self::GridHistoryForward => "grid_history_forward",
            Self::GridParentFolder => "grid_parent_folder",
            Self::ImageRotateLeft => "image_rotate_left",
            Self::ImageRotateRight => "image_rotate_right",
            Self::ImageCapture => "image_capture",
            Self::ImageToggleMetadata => "image_toggle_metadata",
            Self::ImageSlideshow => "image_slideshow",
            Self::ImagePixelGrid => "image_pixel_grid",
            Self::ImageBackgroundCycle => "image_background_cycle",
            Self::ImageComparePin => "image_compare_pin",
            Self::VideoCapture => "video_capture",
            Self::VideoMute => "video_mute",
            Self::VideoLoop => "video_loop",
            Self::VideoBookmark => "video_bookmark",
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
            "cycle_favorite" => Self::CycleFavorite,
            "grid_toggle_details" => Self::GridToggleDetails,
            "grid_toggle_snapshot_lock" => Self::GridToggleSnapshotLock,
            "grid_toggle_check" => Self::GridToggleCheck,
            "grid_select_all" => Self::GridSelectAll,
            "grid_history_back" => Self::GridHistoryBack,
            "grid_history_forward" => Self::GridHistoryForward,
            "grid_parent_folder" => Self::GridParentFolder,
            "image_rotate_left" => Self::ImageRotateLeft,
            "image_rotate_right" => Self::ImageRotateRight,
            "image_capture" => Self::ImageCapture,
            "image_toggle_metadata" => Self::ImageToggleMetadata,
            "image_slideshow" => Self::ImageSlideshow,
            "image_pixel_grid" => Self::ImagePixelGrid,
            "image_background_cycle" => Self::ImageBackgroundCycle,
            "image_compare_pin" => Self::ImageComparePin,
            "video_capture" => Self::VideoCapture,
            "video_mute" => Self::VideoMute,
            "video_loop" => Self::VideoLoop,
            "video_bookmark" => Self::VideoBookmark,
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
            Self::CycleFavorite => "お気に入り巡回",
            Self::GridToggleDetails => "表示/詳細",
            Self::GridToggleSnapshotLock => "★固定",
            Self::GridToggleCheck => "チェック ON/OFF",
            Self::GridSelectAll => "全選択",
            Self::GridHistoryBack => "フォルダ履歴 戻る",
            Self::GridHistoryForward => "フォルダ履歴 進む",
            Self::GridParentFolder => "親フォルダへ",
            Self::ImageRotateLeft => "回転 L",
            Self::ImageRotateRight => "回転 R",
            Self::ImageCapture => "キャプチャ保存",
            Self::ImageToggleMetadata => "メタデータ表示",
            Self::ImageSlideshow => "スライドショー",
            Self::ImagePixelGrid => "ピクセルグリッド",
            Self::ImageBackgroundCycle => "背景色サイクル",
            Self::ImageComparePin => "比較ピン",
            Self::VideoCapture => "キャプチャ保存",
            Self::VideoMute => "ミュート",
            Self::VideoLoop => "ループ",
            Self::VideoBookmark => "ブックマーク追加",
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
                    | Self::AddToBook
                    | Self::PinRepresentativeThumb
                    | Self::ToggleDetachedViewer
                    | Self::CycleFavorite
                    | Self::GridToggleDetails
                    | Self::GridToggleSnapshotLock
                    | Self::GridToggleCheck
                    | Self::GridSelectAll
                    | Self::GridHistoryBack
                    | Self::GridHistoryForward
                    | Self::GridParentFolder
            ),
            RingShortcutContext::ImageFullscreen => matches!(
                self,
                Self::None
                    | Self::AddToBook
                    | Self::PinRepresentativeThumb
                    | Self::ToggleDetachedViewer
                    | Self::CycleFavorite
                    | Self::ImageRotateLeft
                    | Self::ImageRotateRight
                    | Self::ImageCapture
                    | Self::ImageToggleMetadata
                    | Self::ImageSlideshow
                    | Self::ImagePixelGrid
                    | Self::ImageBackgroundCycle
                    | Self::ImageComparePin
            ),
            RingShortcutContext::VideoFullscreen => matches!(
                self,
                Self::None
                    | Self::AddToBook
                    | Self::PinRepresentativeThumb
                    | Self::ToggleDetachedViewer
                    | Self::CycleFavorite
                    | Self::VideoCapture
                    | Self::VideoMute
                    | Self::VideoLoop
                    | Self::VideoBookmark
                    | Self::VideoTileMode
                    | Self::VideoExternalPlayer
            ),
        }
    }

    pub fn available_for_context(context: RingShortcutContext) -> Vec<Self> {
        let actions = match context {
            RingShortcutContext::Grid => vec![
                Self::None,
                Self::AddToBook,
                Self::PinRepresentativeThumb,
                Self::ToggleDetachedViewer,
                Self::CycleFavorite,
                Self::GridToggleDetails,
                Self::GridToggleSnapshotLock,
                Self::GridToggleCheck,
                Self::GridSelectAll,
                Self::GridHistoryBack,
                Self::GridHistoryForward,
                Self::GridParentFolder,
            ],
            RingShortcutContext::ImageFullscreen => vec![
                Self::None,
                Self::AddToBook,
                Self::PinRepresentativeThumb,
                Self::ToggleDetachedViewer,
                Self::CycleFavorite,
                Self::ImageRotateLeft,
                Self::ImageRotateRight,
                Self::ImageCapture,
                Self::ImageToggleMetadata,
                Self::ImageSlideshow,
                Self::ImagePixelGrid,
                Self::ImageBackgroundCycle,
                Self::ImageComparePin,
            ],
            RingShortcutContext::VideoFullscreen => vec![
                Self::None,
                Self::AddToBook,
                Self::PinRepresentativeThumb,
                Self::ToggleDetachedViewer,
                Self::CycleFavorite,
                Self::VideoCapture,
                Self::VideoMute,
                Self::VideoLoop,
                Self::VideoBookmark,
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
    #[serde(default)]
    pub mouse_flick_enabled: bool,
    #[serde(default = "default_true")]
    pub gamepad_ring_enabled: bool,
    #[serde(default)]
    pub shift_wheel_pair: WheelPairActionId,
    #[serde(default)]
    pub alt_wheel_pair: WheelPairActionId,
    #[serde(default)]
    pub mouse_back_forward_action: MouseBackForwardActionId,
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

    pub fn sanitize(&mut self) {
        self.grid.sanitize(RingShortcutContext::Grid);
        self.image.sanitize(RingShortcutContext::ImageFullscreen);
        self.video.sanitize(RingShortcutContext::VideoFullscreen);
        if matches!(self.shift_wheel_pair, WheelPairActionId::Unknown(_)) {
            self.shift_wheel_pair = WheelPairActionId::None;
        }
        if matches!(self.alt_wheel_pair, WheelPairActionId::Unknown(_)) {
            self.alt_wheel_pair = WheelPairActionId::None;
        }
        if matches!(
            self.mouse_back_forward_action,
            MouseBackForwardActionId::Unknown(_)
        ) {
            self.mouse_back_forward_action = MouseBackForwardActionId::None;
        }
    }
}

impl Default for RingShortcutSettings {
    fn default() -> Self {
        Self {
            mouse_flick_enabled: false,
            gamepad_ring_enabled: true,
            shift_wheel_pair: WheelPairActionId::None,
            alt_wheel_pair: WheelPairActionId::None,
            mouse_back_forward_action: MouseBackForwardActionId::None,
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

fn default_grid_profile() -> RingShortcutProfile {
    RingShortcutProfile::new(vec![
        RingActionId::AddToBook,
        RingActionId::GridToggleSnapshotLock,
        RingActionId::GridHistoryForward,
        RingActionId::None,
        RingActionId::PinRepresentativeThumb,
        RingActionId::GridToggleCheck,
        RingActionId::GridHistoryBack,
        RingActionId::GridToggleDetails,
    ])
}

fn default_image_profile() -> RingShortcutProfile {
    RingShortcutProfile::new(vec![
        RingActionId::AddToBook,
        RingActionId::ImageToggleMetadata,
        RingActionId::ImageRotateRight,
        RingActionId::None,
        RingActionId::PinRepresentativeThumb,
        RingActionId::ImageSlideshow,
        RingActionId::ImageRotateLeft,
        RingActionId::ImageCapture,
    ])
}

fn default_video_profile() -> RingShortcutProfile {
    RingShortcutProfile::new(vec![
        RingActionId::AddToBook,
        RingActionId::VideoLoop,
        RingActionId::VideoCapture,
        RingActionId::VideoBookmark,
        RingActionId::PinRepresentativeThumb,
        RingActionId::VideoTileMode,
        RingActionId::VideoMute,
        RingActionId::VideoExternalPlayer,
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
    pub original: RingPickerOriginalState,
    pub row: usize,
    pub dirty_rows: Vec<RingPickerRowId>,
    pub x_close_armed: bool,
    pub drill: Option<PostFilterDrillState>,
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
pub enum PostFilterDrillMode {
    Group,
    Item,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostFilterDrillState {
    pub mode: PostFilterDrillMode,
    pub group: usize,
    pub item: usize,
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
        self.armed || self.elapsed() >= mouse_flick_guide_delay()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MouseFlickOutcome {
    None,
    ShortTap,
    LongPressMenu(egui::Pos2),
    Fired,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profiles_match_design_slots() {
        let defaults = RingShortcutSettings::default();
        assert_eq!(defaults.mouse_flick_enabled, false);
        assert_eq!(defaults.gamepad_ring_enabled, true);
        assert_eq!(defaults.shift_wheel_pair, WheelPairActionId::None);
        assert_eq!(defaults.alt_wheel_pair, WheelPairActionId::None);
        assert_eq!(
            defaults.mouse_back_forward_action,
            MouseBackForwardActionId::None
        );
        assert_eq!(defaults.mouse_nav_prompt_done, false);
        assert_eq!(defaults.x_picker_hint_shown, false);
        assert_eq!(
            defaults.grid.slots[RingDirection::Up.slot_index()],
            RingActionId::AddToBook
        );
        assert_eq!(
            defaults.image.slots[RingDirection::UpRight.slot_index()],
            RingActionId::ImageToggleMetadata
        );
        assert_eq!(
            defaults.video.slots[RingDirection::DownRight.slot_index()],
            RingActionId::VideoBookmark
        );
    }

    #[test]
    fn sanitize_clears_unknown_and_wrong_context_actions() {
        let mut settings = RingShortcutSettings::default();
        settings.grid.slots[0] = RingActionId::Unknown("future_action".to_string());
        settings.image.slots[1] = RingActionId::VideoCapture;
        settings.video.slots.push(RingActionId::ImageCapture);
        settings.shift_wheel_pair = WheelPairActionId::Unknown("future_wheel".to_string());
        settings.mouse_back_forward_action =
            MouseBackForwardActionId::Unknown("future_mouse_nav".to_string());

        settings.sanitize();

        assert_eq!(settings.grid.slots[0], RingActionId::None);
        assert_eq!(settings.image.slots[1], RingActionId::None);
        assert_eq!(settings.video.slots.len(), RING_SHORTCUT_SLOT_COUNT);
        assert_eq!(settings.shift_wheel_pair, WheelPairActionId::None);
        assert_eq!(
            settings.mouse_back_forward_action,
            MouseBackForwardActionId::None
        );
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
