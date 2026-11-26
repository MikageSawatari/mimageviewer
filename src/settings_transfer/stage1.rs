use super::*;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Stage1Choice {
    pub value: Value,
    pub label: String,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Stage1Row {
    pub key: String,
    pub label: String,
    pub value: Value,
    pub choices: Option<Vec<Stage1Choice>>,
    pub template: Option<Value>,
    pub overridden_by_favorite: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct Stage1Change {
    pub key: String,
    pub label: String,
    pub before: Value,
    pub after: Value,
    pub lines: Vec<(String, String, String)>,
}

pub(crate) const PARALLELISM_MAX: usize = 64;
pub(crate) const RAW_PARALLELISM_MAX: u8 = 10;
pub(crate) const VST3_CHAIN_MAX_LEN: usize = 10;
pub(crate) const PREFETCH_MAX: usize = 50;
pub(crate) const THUMB_PREFETCH_MAX: u32 = 20;
pub(crate) const AI_PREFETCH_MAX: usize = 10;
pub(crate) const GPU_MEMORY_PERCENT_MAX: u32 = 100;
pub(crate) const CACHE_THRESHOLD_MIN_MS: u32 = 10;
pub(crate) const CACHE_THRESHOLD_MAX_MS: u32 = 100;
pub(crate) const CACHE_THRESHOLD_MIN_BYTES: u64 = 500_000;
pub(crate) const CACHE_THRESHOLD_MAX_BYTES: u64 = 10_000_000;
pub(crate) const EDIT_CACHE_MIN_BYTES: u64 = 100_000_000;
pub(crate) const EDIT_CACHE_MAX_BYTES: u64 = 100_000_000_000;
pub(crate) const ARCHIVE_CACHE_MIN_BYTES: u64 = 1_000_000;
pub(crate) const ARCHIVE_CACHE_MAX_BYTES: u64 = 1_000_000_000_000;
pub(crate) const AI_SIZE_OPTIONS: &[(u32, u32)] = &[
    (512, 512),
    (1024, 1024),
    (2048, 1024),
    (2048, 2048),
    (4096, 2048),
    (4096, 4096),
    (6144, 4096),
    (8192, 4096),
    (8192, 8192),
];

/// Candidates and user-facing rendering belong to the type. Every export type must implement this trait.
pub(super) trait Stage1Value: Serialize + DeserializeOwned + Clone {
    fn candidates() -> Option<Vec<(Self, String)>> {
        None
    }
    fn template() -> Option<Value> {
        None
    }
    fn change_lines(before: &Self, after: &Self) -> Vec<(String, String, String)> {
        vec![(String::new(), typed_readable(before), typed_readable(after))]
    }
}
macro_rules! no_choices { ($($ty:ty),* $(,)?) => { $(impl Stage1Value for $ty {})* }; }
macro_rules! enum_choices { ($ty:ty; $($variant:ident => $label:literal),*) => {
    impl Stage1Value for $ty { fn candidates() -> Option<Vec<(Self,String)>> { Some(vec![$((Self::$variant,$label.into())),*]) } }
}; }
impl<T: Serialize + DeserializeOwned + Clone + 'static> Stage1Value for Vec<T> {
    fn template() -> Option<Value> {
        (std::any::TypeId::of::<T>()
            == std::any::TypeId::of::<crate::external_tool::ExternalTool>())
        .then(external_tools_template)
    }
    fn change_lines(before: &Self, after: &Self) -> Vec<(String, String, String)> {
        if std::any::TypeId::of::<T>()
            == std::any::TypeId::of::<crate::external_tool::ExternalTool>()
        {
            let before = serde_json::from_value(serde_json::to_value(before).unwrap()).unwrap();
            let after = serde_json::from_value(serde_json::to_value(after).unwrap()).unwrap();
            return external_tool_lines(&before, &after);
        }
        vec![(
            String::new(),
            readable(&serde_json::to_value(before).unwrap()),
            readable(&serde_json::to_value(after).unwrap()),
        )]
    }
}
impl<T: Serialize + DeserializeOwned + Clone> Stage1Value for Option<T> {}

pub(super) fn stage1_row<T: Stage1Value>(
    key: &str,
    label: &str,
    value: &T,
    check: impl Fn(&T, &Value) -> bool,
) -> Stage1Row {
    let choices = T::candidates().map(|values| {
        values
            .into_iter()
            .filter_map(|(value, label)| {
                let raw = serde_json::to_value(&value).unwrap();
                validated::<T>(&raw, &check).map(|_| Stage1Choice { value: raw, label })
            })
            .collect()
    });
    Stage1Row {
        key: key.into(),
        label: label.into(),
        value: serde_json::to_value(value).unwrap(),
        choices,
        template: T::template(),
        overridden_by_favorite: false,
    }
}
pub(super) fn stage1_decode<T: Serialize + DeserializeOwned>(
    _: &T,
    raw: &Value,
    check: impl FnOnce(&T, &Value) -> bool,
) -> Option<T> {
    // Only @prefs rows use this decoder. Export keys go through parse_items,
    // including its key-specific accepts_null policy. Nullable @prefs values
    // (for example resetting an output folder) retain their UI validators.
    validated(raw, check)
}
pub(super) fn unique_strings(v: &Vec<String>) -> bool {
    v.iter().all(|s| !s.trim().is_empty()) && v.iter().collect::<HashSet<_>>().len() == v.len()
}
pub(super) fn valid_ai_backend(value: &Option<String>, _: &Value) -> bool {
    use crate::ai::AiBackend;
    value.as_deref().is_none_or(|name| {
        matches!(
            AiBackend::from_str(name),
            Some(AiBackend::DirectMl | AiBackend::TensorRt)
        )
    })
}
pub(super) fn stage1_vst3_paths(settings: &Settings) -> Vec<String> {
    settings
        .vst3_plugins
        .iter()
        .map(|entry| entry.path.clone())
        .collect()
}
pub(super) fn valid_vst3_paths(paths: &Vec<String>, _: &Value) -> bool {
    paths.len() <= VST3_CHAIN_MAX_LEN
        && unique_strings(paths)
        && paths.iter().all(|path| absolute(Path::new(path)))
}
pub(super) fn stage1_set_vst3_paths(settings: &mut Settings, paths: &[String]) {
    let mut existing = HashMap::with_capacity(settings.vst3_plugins.len());
    for entry in &settings.vst3_plugins {
        // Match Preferences' first matching live entry even for legacy duplicates.
        existing.entry(entry.path.as_str()).or_insert(entry);
    }
    let entries = paths
        .iter()
        .map(|path| {
            existing
                .get(path.as_str())
                .map(|entry| (**entry).clone())
                .unwrap_or_else(|| Vst3PluginEntry {
                    path: path.clone(),
                    ..Default::default()
                })
        })
        .collect();
    settings.vst3_plugins = entries;
}
pub(super) fn valid_ai_limit(
    v: &Option<crate::ai::upscale::AiProcessSizeLimit>,
    _: &Value,
) -> bool {
    v.as_ref()
        .is_none_or(|v| AI_SIZE_OPTIONS.contains(&(v.long_edge_px, v.short_edge_px)))
}
pub(super) fn valid_menu_layout(value: &crate::keymap::MenuLayoutSettings, _: &Value) -> bool {
    use crate::keymap::*;
    unique_strings(&value.top_menu_order)
        && value
            .top_menu_order
            .iter()
            .all(|name| TopMenuId::parse_stable_name(name).is_some())
        && unique_strings(&value.hidden_commands)
        && value.hidden_commands.iter().all(|name| {
            MenuCommandId::parse_stable_name(name).is_some_and(menu_command_can_be_hidden)
        })
        && value
            .command_order
            .iter()
            .map(|order| &order.parent)
            .collect::<HashSet<_>>()
            .len()
            == value.command_order.len()
        && value.command_order.iter().all(|order| {
            TopMenuId::parse_stable_name(&order.parent).is_some_and(|parent| {
                unique_strings(&order.commands)
                    && order.commands.iter().all(|name| {
                        MenuCommandId::parse_stable_name(name)
                            .and_then(menu_command_spec)
                            .is_some_and(|spec| spec.parent == parent)
                    })
            })
        })
}
pub(super) fn valid_context_menu_layout(
    value: &crate::context_menu_model::ContextMenuLayoutSettings,
    _: &Value,
) -> bool {
    use crate::context_menu_model::*;
    unique_strings(&value.hidden_items)
        && value
            .hidden_items
            .iter()
            .all(|name| ContextMenuItemId::parse_stable_name(name).is_some())
        && value
            .order
            .iter()
            .map(|order| &order.parent)
            .collect::<HashSet<_>>()
            .len()
            == value.order.len()
        && value.order.iter().all(|order| {
            ContextMenuParentId::parse_stable_name(&order.parent).is_some_and(|parent| {
                unique_strings(&order.items)
                    && order.items.iter().all(|name| {
                        ContextMenuItemId::parse_stable_name(name)
                            .is_some_and(|item| item.parent() == parent)
                    })
            })
        })
        && value
            .separators
            .iter()
            .map(|separator| &separator.item)
            .collect::<HashSet<_>>()
            .len()
            == value.separators.len()
        && value
            .separators
            .iter()
            .all(|separator| ContextMenuItemId::parse_stable_name(&separator.item).is_some())
}
pub(super) fn stage1_video_lock(s: &Settings) -> StillLock {
    match s.video_bottom_lock() {
        BottomBarLock::None => StillLock::None,
        BottomBarLock::BarOnly => StillLock::BarOnly,
        BottomBarLock::BarAndStrip => StillLock::BarAndStrip,
    }
}
pub(super) fn stage1_set_video_lock(s: &mut Settings, v: &StillLock) {
    s.set_video_bottom_lock(match v {
        StillLock::None => BottomBarLock::None,
        StillLock::BarOnly => BottomBarLock::BarOnly,
        StillLock::BarAndStrip => BottomBarLock::BarAndStrip,
    });
}

pub(super) fn stage1_key_error(key: &str, reason: &str) -> String {
    if matches!(key, "keymap" | "ring_shortcuts" | "gamepad_enabled") {
        return format!("{key}: set_operations で変えます");
    }
    if key.starts_with("agent_")
        || matches!(
            key,
            "pin" | "remote_pin" | "remote_web_pin" | "remote-web-auth.json"
        )
    {
        format!("{key}: AI からは変えられません")
    } else if reason == "不明な設定" {
        format!("{key}: 不明な設定です")
    } else {
        format!("{key}: この設定は、まだ AI からは変えられません ({reason})")
    }
}
no_choices!(
    FullscreenChromeSuppression,
    FullscreenSpecifiedScale,
    GridDisplayOrder,
    StillSeekPreviewSizeValues,
    StillSeekStripHeightValues,
    VideoSeekPreviewSizeValues,
    [u8; 3],
    bool,
    crate::details_name_colors::DetailsNameColors,
    crate::folder_cell_background::FolderCellBackground,
    crate::file_type_visibility::FileTypeVisibility,
    crate::grid_item::ContainerIconKinds,
    crate::video::seek_strip_layout::SeekStripCycleSet,
    crate::video::seek_strip_layout::SeekStripHeightValues,
    f32,
    f64,
    u32,
    usize,
    StillChrome,
    Parallelism,
    std::path::PathBuf,
    String,
    u8,
    u64,
    crate::keymap::MenuLayoutSettings,
    crate::context_menu_model::ContextMenuLayoutSettings,
    FileOrganizeDestination,
    crate::external_tool::ExternalTool
);
impl Stage1Value for AiFeatureMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Disabled, Self::Disabled.label().into()),
            (Self::Light, Self::Light.label().into()),
            (Self::HighQuality, Self::HighQuality.label().into()),
        ])
    }
}
impl Stage1Value for ArchiveFileHandling {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Legacy, Self::Legacy.label().into()),
            (Self::Ask, Self::Ask.label().into()),
            (Self::Convert, Self::Convert.label().into()),
            (Self::Ignore, Self::Ignore.label().into()),
        ])
    }
}
impl Stage1Value for AudioThumbnailIndicator {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::MusicNoteIcon, Self::MusicNoteIcon.label().into()),
            (Self::BottomLeftBadge, Self::BottomLeftBadge.label().into()),
            (Self::Hidden, Self::Hidden.label().into()),
        ])
    }
}
impl Stage1Value for EpubFileHandling {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Ask, Self::Ask.label().into()),
            (Self::Convert, Self::Convert.label().into()),
            (Self::Ignore, Self::Ignore.label().into()),
        ])
    }
}
impl Stage1Value for FsSidePanelMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Hover, Self::Hover.label().into()),
            (Self::ClickToShow, Self::ClickToShow.label().into()),
        ])
    }
}
impl Stage1Value for FullscreenFitMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Page, Self::Page.label().into()),
            (Self::MarginFit, Self::MarginFit.label().into()),
            (Self::Width, Self::Width.label().into()),
            (Self::Height, Self::Height.label().into()),
            (Self::Original, Self::Original.label().into()),
            (Self::SpecifiedScale, Self::SpecifiedScale.label().into()),
        ])
    }
}
impl Stage1Value for FullscreenHorizontalCursorDirection {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::FollowPage, Self::FollowPage.label().into()),
            (Self::FollowSeekBar, Self::FollowSeekBar.label().into()),
        ])
    }
}
impl Stage1Value for FullscreenJumpMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Percent, Self::Percent.label().into()),
            (Self::FixedPages, Self::FixedPages.label().into()),
        ])
    }
}
impl Stage1Value for FullscreenNScrollOrder {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::VerticalFirst, Self::VerticalFirst.label().into()),
            (Self::HorizontalFirst, Self::HorizontalFirst.label().into()),
        ])
    }
}
impl Stage1Value for FullscreenPageStartPosition {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Center, Self::Center.label().into()),
            (Self::ReadingStart, Self::ReadingStart.label().into()),
        ])
    }
}
impl Stage1Value for FullscreenSeekDirection {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::FollowReading, Self::FollowReading.label().into()),
            (Self::LeftToRight, Self::LeftToRight.label().into()),
        ])
    }
}
impl Stage1Value for FullscreenUpDownKeyAction {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::PageTurn, Self::PageTurn.label().into()),
            (Self::InPageScroll, Self::InPageScroll.label().into()),
        ])
    }
}
impl Stage1Value for FullscreenWheelAction {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::PageTurn, Self::PageTurn.label().into()),
            (Self::InPageScroll, Self::InPageScroll.label().into()),
            (
                Self::ScrollThenPageTurn,
                Self::ScrollThenPageTurn.label().into(),
            ),
            (Self::Zoom, Self::Zoom.label().into()),
        ])
    }
}
impl Stage1Value for FullscreenWheelZoomStep {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(
            Self::all()
                .iter()
                .map(|&step| (step, step.label().into()))
                .collect(),
        )
    }
}
impl Stage1Value for GridClickSelectionMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Check, Self::Check.label().into()),
            (Self::Explorer, Self::Explorer.label().into()),
        ])
    }
}
impl Stage1Value for NotificationDuration {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Short, Self::Short.label().into()),
            (Self::Standard, Self::Standard.label().into()),
            (Self::Long, Self::Long.label().into()),
        ])
    }
}
impl Stage1Value for FullscreenPrimaryClickMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        // 表示名は環境設定のラジオボタンと同じ。
        Some(vec![
            (Self::PageTurn, "クリックでページを送る (既定)".into()),
            (
                Self::DoubleClickClose,
                "クリックでは送らず、ダブルクリックで閉じる".into(),
            ),
        ])
    }
}
impl Stage1Value for PageTurnAnimationStyle {
    fn candidates() -> Option<Vec<(Self, String)>> {
        // 表示名は環境設定のラジオボタンと同じ。
        Some(vec![
            (Self::Curl, "めくる".into()),
            (Self::Slide, "スライド".into()),
        ])
    }
}
impl Stage1Value for ReadingDirection {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Ltr, Self::Ltr.label().into()),
            (Self::Rtl, Self::Rtl.label().into()),
        ])
    }
}
impl Stage1Value for ReadingFlow {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Paged, Self::Paged.label().into()),
            (Self::Vertical, Self::Vertical.label().into()),
            (Self::Horizontal, Self::Horizontal.label().into()),
        ])
    }
}
impl Stage1Value for ResumeMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Resume, Self::Resume.label().into()),
            (Self::FromStart, Self::FromStart.label().into()),
        ])
    }
}
impl Stage1Value for SelectionInfoDisplayMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Tooltip, Self::Tooltip.label().into()),
            (Self::BottomBar, Self::BottomBar.label().into()),
            (Self::Both, Self::Both.label().into()),
            (Self::Hidden, Self::Hidden.label().into()),
        ])
    }
}
impl Stage1Value for SlideshowEndAction {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::LoopFolder, Self::LoopFolder.label().into()),
            (Self::NextFolder, Self::NextFolder.label().into()),
            (Self::Stop, Self::Stop.label().into()),
        ])
    }
}
impl Stage1Value for SortOrder {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::FileName, Self::FileName.label().into()),
            (Self::FileNameDesc, Self::FileNameDesc.label().into()),
            (Self::Numeric, Self::Numeric.label().into()),
            (Self::NumericDesc, Self::NumericDesc.label().into()),
            (Self::DateAsc, Self::DateAsc.label().into()),
            (Self::DateDesc, Self::DateDesc.label().into()),
            (Self::SizeAsc, Self::SizeAsc.label().into()),
            (Self::SizeDesc, Self::SizeDesc.label().into()),
            (Self::DurationAsc, Self::DurationAsc.label().into()),
            (Self::DurationDesc, Self::DurationDesc.label().into()),
            (Self::PageCountAsc, Self::PageCountAsc.label().into()),
            (Self::PageCountDesc, Self::PageCountDesc.label().into()),
            (Self::RatingAsc, Self::RatingAsc.label().into()),
            (Self::RatingDesc, Self::RatingDesc.label().into()),
        ])
    }
}
impl Stage1Value for SpreadMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Single, Self::Single.label().into()),
            (Self::Ltr, Self::Ltr.label().into()),
            (Self::LtrCover, Self::LtrCover.label().into()),
            (Self::Rtl, Self::Rtl.label().into()),
            (Self::RtlCover, Self::RtlCover.label().into()),
            (Self::Vertical, Self::Vertical.label().into()),
            (Self::SplitLtr, Self::SplitLtr.label().into()),
            (Self::SplitRtl, Self::SplitRtl.label().into()),
        ])
    }
}
impl Stage1Value for StartupWindowState {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::RememberLast, Self::RememberLast.label().into()),
            (Self::Normal, Self::Normal.label().into()),
            (Self::Maximized, Self::Maximized.label().into()),
        ])
    }
}
impl Stage1Value for StillSeekBarWithStrip {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Show, Self::Show.label().into()),
            (Self::Hide, Self::Hide.label().into()),
        ])
    }
}
impl Stage1Value for StillSeekHoverPreviewMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Always, Self::Always.label().into()),
            (
                Self::HideWithThumbnailStrip,
                Self::HideWithThumbnailStrip.label().into(),
            ),
            (Self::Never, Self::Never.label().into()),
        ])
    }
}
impl Stage1Value for StillSeekPreviewSize {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Maximum, Self::Maximum.label().into()),
            (Self::Large, Self::Large.label().into()),
            (Self::Medium, Self::Medium.label().into()),
            (Self::Small, Self::Small.label().into()),
            (Self::Smallest, Self::Smallest.label().into()),
        ])
    }
}
impl Stage1Value for StillSeekStripHeight {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Maximum, Self::Maximum.label().into()),
            (Self::Large, Self::Large.label().into()),
            (Self::Medium, Self::Medium.label().into()),
            (Self::Small, Self::Small.label().into()),
            (Self::Smallest, Self::Smallest.label().into()),
        ])
    }
}
impl Stage1Value for TextContrast {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Standard, Self::Standard.label().into()),
            (Self::Strong, Self::Strong.label().into()),
        ])
    }
}
impl Stage1Value for UiTheme {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::System, Self::System.label().into()),
            (Self::Standard, Self::Standard.label().into()),
            (Self::Light, Self::Light.label().into()),
            (Self::Dark, Self::Dark.label().into()),
        ])
    }
}
impl Stage1Value for VideoLoopMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Off, Self::Off.label().into()),
            (Self::Full, Self::Full.label().into()),
            (Self::Chapter, Self::Chapter.label().into()),
            (Self::Bookmark, Self::Bookmark.label().into()),
        ])
    }
}
impl Stage1Value for VideoSeekBarWithStrip {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Show, Self::Show.label().into()),
            (Self::Hide, Self::Hide.label().into()),
        ])
    }
}
impl Stage1Value for VideoSeekHoverPreviewMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Always, Self::Always.label().into()),
            (
                Self::HideWithThumbnailStrip,
                Self::HideWithThumbnailStrip.label().into(),
            ),
            (Self::Never, Self::Never.label().into()),
        ])
    }
}
impl Stage1Value for VideoSeekPreviewSize {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Maximum, Self::Maximum.label().into()),
            (Self::Large, Self::Large.label().into()),
            (Self::Medium, Self::Medium.label().into()),
            (Self::Small, Self::Small.label().into()),
            (Self::Smallest, Self::Smallest.label().into()),
        ])
    }
}
impl Stage1Value for VideoThumbnailIndicator {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::PlayIcon, Self::PlayIcon.label().into()),
            (Self::BottomLeftBadge, Self::BottomLeftBadge.label().into()),
            (Self::Hidden, Self::Hidden.label().into()),
        ])
    }
}
impl Stage1Value for crate::bake_stage::BakeStage {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Edits, Self::Edits.label().into()),
            (Self::Ai, Self::Ai.label().into()),
            (Self::DisplayAdjust, Self::DisplayAdjust.label().into()),
        ])
    }
}
impl Stage1Value for crate::capture::CaptureFormat {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Png, Self::Png.label().into()),
            (Self::Jpeg95, Self::Jpeg95.label().into()),
            (Self::Jpeg85, Self::Jpeg85.label().into()),
            (Self::Jpeg75, Self::Jpeg75.label().into()),
        ])
    }
}
impl Stage1Value for crate::panorama::PanoProjection {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Perspective, Self::Perspective.label().into()),
            (Self::Stereographic, Self::Stereographic.label().into()),
            (Self::Equidistant, Self::Equidistant.label().into()),
            (Self::EquisolidAngle, Self::EquisolidAngle.label().into()),
        ])
    }
}
enum_choices!(crate::rating_sort::RatingSortUnratedPosition; BetweenThreeAndTwo => "未評価は★3と★2の間",BelowAll => "未評価を最後");
enum_choices!(crate::raw::RawBrightness; MatchPreview => "プレビューに合わせる",None => "補正しない");
enum_choices!(crate::same_name_folder_book::SameNameFolderBookPolicy; PreferBook => "本を表示",PreferFolder => "フォルダを表示",ShowBoth => "両方表示");
impl Stage1Value for crate::video::seek_strip_layout::SeekStripHeight {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Maximum, Self::Maximum.label().into()),
            (Self::Large, Self::Large.label().into()),
            (Self::Medium, Self::Medium.label().into()),
            (Self::Small, Self::Small.label().into()),
            (Self::Smallest, Self::Smallest.label().into()),
        ])
    }
}
impl Stage1Value for DetailsSelectionBarMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::SameAsDetails, Self::SameAsDetails.label().into()),
            (Self::Dedicated, Self::Dedicated.label().into()),
            (Self::Hidden, Self::Hidden.label().into()),
        ])
    }
}
impl Stage1Value for StartupFolderMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Previous, Self::Previous.label().into()),
            (Self::Desktop, Self::Desktop.label().into()),
            (Self::Specific, Self::Specific.label().into()),
            (Self::Drives, Self::Drives.label().into()),
            (Self::ReadingHistory, Self::ReadingHistory.label().into()),
        ])
    }
}
impl Stage1Value for CachePolicy {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Off, Self::Off.label().into()),
            (Self::Auto, Self::Auto.label().into()),
            (Self::Always, Self::Always.label().into()),
        ])
    }
}
impl Stage1Value for IndexerSpeedProfile {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Low, Self::Low.label().into()),
            (Self::Medium, Self::Medium.label().into()),
            (Self::High, Self::High.label().into()),
        ])
    }
}
impl Stage1Value for AnimeUpscaleSourceLimit {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Px2048, Self::Px2048.label().into()),
            (Self::Px4096, Self::Px4096.label().into()),
            (Self::Unlimited, Self::Unlimited.label().into()),
        ])
    }
}
impl Stage1Value for VideoDeinterlaceMode {
    fn candidates() -> Option<Vec<(Self, String)>> {
        Some(vec![
            (Self::Auto, Self::Auto.label().into()),
            (Self::On, Self::On.label().into()),
            (Self::Off, Self::Off.label().into()),
        ])
    }
}
enum_choices!(StillLock; None => "固定しない",BarOnly => "バーだけ固定",BarAndStrip => "バーと列を固定");

pub(super) fn stage1_lines<T: Stage1Value>(before: &T, after: &T) -> Vec<(String, String, String)> {
    T::change_lines(before, after)
}
fn typed_readable<T: Stage1Value>(value: &T) -> String {
    let raw = serde_json::to_value(value).unwrap();
    T::candidates()
        .and_then(|choices| {
            choices
                .into_iter()
                .find(|(v, _)| serde_json::to_value(v).unwrap() == raw)
                .map(|(_, label)| label)
        })
        .unwrap_or_else(|| readable(&raw))
}
fn readable(raw: &Value) -> String {
    match raw {
        Value::Null => "既定".into(),
        Value::Bool(value) => if *value { "オン" } else { "オフ" }.into(),
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Array(values) => values.iter().map(readable).collect::<Vec<_>>().join("、"),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| format!("{key}: {}", readable(value)))
            .collect::<Vec<_>>()
            .join("、"),
    }
}

fn external_tools_template() -> Value {
    use crate::external_tool::*;
    let mut example = serde_json::to_value(ExternalTool::defaults_for_viewing()).unwrap();
    example.as_object_mut().unwrap().remove("id");
    serde_json::json!({"example": example, "choices": {
        "launch": [{"value":{"Executable":"C:\\Tools\\app.exe"},"label":"実行ファイル"},{"value":"OsDefault","label":"OS の関連付け"}],
        "payload": [{"value":"TempEdited","label":"一時ファイル (編集を反映)"},{"value":"TempOriginal","label":"一時ファイル (編集前)"},{"value":"OriginalFile","label":"元のファイル"}],
        "video": [{"value":"File","label":"ファイル"},{"value":"CurrentFrame","label":"現在のフレーム"}],
        "spread": [{"value":"Merged","label":"合成"},{"value":"BothPages","label":"両ページ"},{"value":"MainPageOnly","label":"主ページのみ"}],
        "selection": [{"value":"Single","label":"1 件"},{"value":"Each","label":"1 件ずつ起動"},{"value":"Batch","label":"まとめて渡す"}]
    }})
}

pub(super) fn stage1_prepare_changes(
    settings: &Settings,
    changes: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    use crate::external_tool::*;
    let mut prepared = changes.clone();
    if let Some(raw) = prepared.get_mut("external_tools") {
        let array = raw
            .as_array_mut()
            .ok_or("external_tools: 一覧を指定してください")?;
        let mut occupied = settings.external_tools.clone();
        let mut used = occupied.iter().map(|tool| tool.id).collect::<HashSet<_>>();
        let existing_associations = settings
            .external_tools
            .iter()
            .filter_map(|tool| {
                if let ExternalToolLaunch::Association { handler_id } = &tool.launch {
                    Some((tool.id, handler_id.as_str()))
                } else {
                    None
                }
            })
            .collect::<HashSet<_>>();
        let mut seen = HashSet::new();
        // Reserve all supplied IDs before assigning IDs to new entries.
        for raw in array.iter() {
            if let Some(id) = raw.get("id") {
                let id: ExternalToolId = serde_json::from_value(id.clone())
                    .map_err(|_| "external_tools: id が正しくありません")?;
                if !seen.insert(id) {
                    return Err("external_tools: id が重複しています".into());
                }
                if used.insert(id) {
                    let mut reserved = ExternalTool::defaults_for_viewing();
                    reserved.id = id;
                    occupied.push(reserved);
                }
            }
        }
        // Reuse the UI's allocation rule once for the complete reserved set.
        // Thereafter each free-ID probe advances monotonically, wrapping once
        // to the first free positive slot if the u32 maximum is reached.
        let mut next = next_id(&occupied).0;
        for raw in array.iter_mut() {
            let object = raw
                .as_object_mut()
                .ok_or("external_tools: 要素の形式が正しくありません")?;
            if !object.contains_key("id") {
                while used.contains(&ExternalToolId(next)) {
                    next = next.checked_add(1).unwrap_or(1);
                }
                let id = ExternalToolId(next);
                used.insert(id);
                next = next.checked_add(1).unwrap_or(1);
                object.insert("id".into(), serde_json::to_value(id).unwrap());
            }
            let mut tool: ExternalTool = validated(&Value::Object(object.clone()), |_, _| true)
                .ok_or("external_tools: 必須項目、型、選択肢が正しくありません")?;
            if let ExternalToolLaunch::Association { handler_id } = &tool.launch {
                if !existing_associations.contains(&(tool.id, handler_id.as_str())) {
                    return Err("external_tools: 関連付けアプリは既存の id と handler_id を維持してください".into());
                }
            }
            // Match the editor's pure value preparation, including TempOriginal:
            // original bytes cannot represent a merged spread.
            tool.spread = tool.effective_spread();
            tool.normalize_for_payload();
            *raw = serde_json::to_value(tool).unwrap();
        }
    }
    Ok(prepared)
}
fn absolute(path: &Path) -> bool {
    path.is_absolute() && !path.to_string_lossy().contains('\0')
}
pub(super) fn stage1_absolute_paths(
    settings: &Settings,
    changes: &Map<String, Value>,
) -> Result<(), String> {
    for (key, path) in [
        ("startup_folder_path", &settings.startup_folder_path),
        ("capture_output_dir", &settings.capture_output_dir),
        ("book_root", &settings.book_root),
        (
            "clipboard_capture_output_dir",
            &settings.clipboard_capture_output_dir,
        ),
    ] {
        if changes.contains_key(key) && path.as_ref().is_some_and(|p| !absolute(p)) {
            return Err(format!("{key}: 絶対パスを指定してください"));
        }
    }
    if changes.contains_key("external_tools") {
        for tool in &settings.external_tools {
            if tool.launch.executable().is_some_and(|p| !absolute(p))
                || tool
                    .working_directory
                    .as_ref()
                    .is_some_and(|p| !absolute(p))
            {
                return Err("external_tools: 絶対パスを指定してください".into());
            }
        }
    }
    Ok(())
}
/// Worker-only hook. The stage-1 path editors do not gate OK on existence;
/// their file pickers/status displays must not become extra AI-only restrictions.
pub(crate) fn validate_stage1_paths(_: &Map<String, Value>) -> Result<(), String> {
    Ok(())
}

fn tool_description(tool: &crate::external_tool::ExternalTool, position: usize) -> String {
    let slot = if position < 10 {
        format!("外部ツール {}", position + 1)
    } else {
        "キー割り当てなし".into()
    };
    format!(
        "{}: {} {} / 渡すもの: {:?} / 位置 {} ({slot})",
        tool.display_name(),
        tool.launch_description(),
        tool.arguments,
        tool.payload,
        position + 1
    )
}
const ASSOCIATION_RESTORE_NOTICE: &str =
    "AI 側からは復元できません。環境設定の画面で登録し直してください";

fn external_tool_lines(
    before: &Vec<crate::external_tool::ExternalTool>,
    after: &Vec<crate::external_tool::ExternalTool>,
) -> Vec<(String, String, String)> {
    let mut lines = Vec::new();
    let mut before_by_id = HashMap::with_capacity(before.len());
    let mut after_by_id = HashMap::with_capacity(after.len());
    for (position, tool) in before.iter().enumerate() {
        before_by_id.entry(tool.id).or_insert((position, tool));
    }
    for (position, tool) in after.iter().enumerate() {
        after_by_id.entry(tool.id).or_insert((position, tool));
    }
    for (position, old) in before.iter().enumerate() {
        match after_by_id.get(&old.id).copied() {
            Some((new_position, new)) if old != new || position != new_position => lines.push((
                format!(
                    "外部ツール{}: {}",
                    if position != new_position {
                        "を移動・変更"
                    } else {
                        "を変更"
                    },
                    new.display_name()
                ),
                tool_description(old, position),
                if matches!(
                    &old.launch,
                    crate::external_tool::ExternalToolLaunch::Association { .. }
                ) && !matches!(
                    &new.launch,
                    crate::external_tool::ExternalToolLaunch::Association { .. }
                ) {
                    format!(
                        "{}。{ASSOCIATION_RESTORE_NOTICE}",
                        tool_description(new, new_position)
                    )
                } else {
                    tool_description(new, new_position)
                },
            )),
            None => lines.push((
                format!("外部ツールを削除: {}", old.display_name()),
                tool_description(old, position),
                if matches!(
                    &old.launch,
                    crate::external_tool::ExternalToolLaunch::Association { .. }
                ) {
                    ASSOCIATION_RESTORE_NOTICE.into()
                } else {
                    String::new()
                },
            )),
            _ => {}
        }
    }
    for (position, new) in after
        .iter()
        .enumerate()
        .filter(|(_, new)| !before_by_id.contains_key(&new.id))
    {
        lines.push((
            format!("外部ツールを追加: {}", new.display_name()),
            String::new(),
            tool_description(new, position),
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn change(key: &str, value: Value) -> Map<String, Value> {
        Map::from_iter([(key.into(), value)])
    }
    fn default_settings() -> Settings {
        let mut settings = Settings::default();
        apply_load_time_migrations(&mut settings);
        settings
    }
    #[test]
    fn stage1_has_every_export_key_and_unique_keys_and_real_anchors() {
        let settings = default_settings();
        let rows = capture_stage1(&settings, false, false);
        let keys = rows
            .iter()
            .map(|row| row.key.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(rows.len(), keys.len());
        let (export, issues) = project(capture_export(&settings));
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(export.len(), 172);
        assert_eq!(STAGE1_MARKED_ANCHORS.len(), 57);
        assert_eq!(rows.len(), 229);
        for key in export.keys() {
            assert!(keys.contains(key.as_str()), "{key}");
        }
        let index = include_str!("../ui_dialogs/preferences/search_index.rs");
        for (key, anchor) in STAGE1_MARKED_ANCHORS {
            assert!(keys.contains(key), "{key}");
            assert!(index.contains(&format!("\"{anchor}\"")), "{key}: {anchor}");
        }
    }
    #[test]
    fn stage1_valid_values_roundtrip_and_every_choice_passes_its_field_validator() {
        let settings = default_settings();
        for row in capture_stage1(&settings, false, false) {
            stage1_validate(&settings, &change(&row.key, row.value.clone()))
                .unwrap_or_else(|error| panic!("{}: {error}", row.key));
            if let Some(choices) = row.choices {
                assert!(!choices.is_empty(), "{}", row.key);
                for choice in choices {
                    assert!(!choice.label.is_empty());
                    stage1_validate(&settings, &change(&row.key, choice.value))
                        .unwrap_or_else(|error| panic!("{}: {error}", row.key));
                }
            }
        }
    }
    #[test]
    fn stage1_enum_candidates_are_filtered_by_the_existing_validator() {
        let rows = capture_stage1(&default_settings(), false, false);
        let choices = |key: &str| {
            rows.iter()
                .find(|r| r.key == key)
                .unwrap()
                .choices
                .as_ref()
                .unwrap()
                .iter()
                .map(|v| v.value.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(choices("folder_thumb_sort").len(), 4);
        assert!(!choices("fullscreen_fit_mode").contains(&json!("MarginFit")));
        assert!(!choices("archive_file_handling").contains(&json!("Legacy")));
        assert_eq!(
            choices("fullscreen_wheel_zoom_step"),
            FullscreenWheelZoomStep::all()
                .iter()
                .map(|step| serde_json::to_value(step).unwrap())
                .collect::<Vec<_>>()
        );
        let zoom = rows
            .iter()
            .find(|row| row.key == "fullscreen_wheel_zoom_step")
            .unwrap();
        for (choice, step) in zoom
            .choices
            .as_ref()
            .unwrap()
            .iter()
            .zip(FullscreenWheelZoomStep::all())
        {
            assert_eq!(choice.label, step.label());
        }
        assert!(
            stage1_validate(
                &default_settings(),
                &change("fullscreen_wheel_zoom_step", json!("Unknown"))
            )
            .is_err()
        );
        let page_turn = rows
            .iter()
            .find(|row| row.key == "page_turn_animation_style")
            .unwrap();
        assert_eq!(
            page_turn
                .choices
                .as_ref()
                .unwrap()
                .iter()
                .map(|choice| (choice.value.clone(), choice.label.as_str()))
                .collect::<Vec<_>>(),
            vec![(json!("curl"), "めくる"), (json!("slide"), "スライド")]
        );
        assert!(
            stage1_validate(
                &default_settings(),
                &change("page_turn_animation_style", json!("Slide"))
            )
            .is_err()
        );
    }
    #[test]
    fn stage1_fullscreen_primary_click_choices_and_validation_match_preferences() {
        let rows = capture_stage1(&default_settings(), false, false);
        let row = rows
            .iter()
            .find(|row| row.key == "fullscreen_primary_click")
            .unwrap();
        assert_eq!(row.value, json!("page_turn"));
        assert_eq!(
            row.choices
                .as_ref()
                .unwrap()
                .iter()
                .map(|choice| (choice.value.clone(), choice.label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (json!("page_turn"), "クリックでページを送る (既定)"),
                (
                    json!("double_click_close"),
                    "クリックでは送らず、ダブルクリックで閉じる"
                ),
            ]
        );
        for (wire, expected) in [
            ("page_turn", FullscreenPrimaryClickMode::PageTurn),
            (
                "double_click_close",
                FullscreenPrimaryClickMode::DoubleClickClose,
            ),
        ] {
            let candidate = stage1_validate(
                &default_settings(),
                &change("fullscreen_primary_click", json!(wire)),
            )
            .unwrap();
            assert_eq!(candidate.fullscreen_primary_click, expected);
        }
        for invalid in [
            json!("future_mode"),
            json!("DoubleClickClose"),
            json!(true),
            json!(0),
            Value::Null,
        ] {
            assert!(
                stage1_validate(
                    &default_settings(),
                    &change("fullscreen_primary_click", invalid),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn stage1_export_null_acceptance_matches_import_for_every_key() {
        let mut settings = default_settings();
        settings.fullscreen_n_scroll_wrap_step_percent = Some(50);
        for key in wire_keys() {
            let changes = change(key, Value::Null);
            let imported = parse_items(&changes);
            let accepts = imported.issues.is_empty() && imported.unknown_count == 0;
            assert_eq!(
                accepts,
                key == "fullscreen_n_scroll_wrap_step_percent",
                "{key}"
            );
            let candidate = stage1_validate(&settings, &changes);
            assert_eq!(candidate.is_ok(), accepts, "{key}");
            if accepts {
                let mut imported_settings = settings.clone();
                imported.apply_to(&mut imported_settings);
                let candidate = candidate.unwrap();
                assert_eq!(candidate.fullscreen_n_scroll_wrap_step_percent, None);
                assert_eq!(
                    candidate.fullscreen_n_scroll_wrap_step_percent,
                    imported_settings.fullscreen_n_scroll_wrap_step_percent
                );
            }
        }
        for (raw, expected) in [
            (json!(10), true),
            (json!(100), true),
            (json!(9), false),
            (json!(101), false),
            (json!(10.5), false),
            (json!("50"), false),
        ] {
            let changes = change("fullscreen_n_scroll_wrap_step_percent", raw);
            assert_eq!(parse_items(&changes).issues.is_empty(), expected);
            assert_eq!(stage1_validate(&settings, &changes).is_ok(), expected);
        }
        let missing = change(
            "update_check_enabled",
            json!(!settings.update_check_enabled),
        );
        let mut imported = settings.clone();
        parse_items(&missing).apply_to(&mut imported);
        let candidate = stage1_validate(&settings, &missing).unwrap();
        assert_eq!(candidate.fullscreen_n_scroll_wrap_step_percent, Some(50));
        assert_eq!(
            candidate.fullscreen_n_scroll_wrap_step_percent,
            imported.fullscreen_n_scroll_wrap_step_percent
        );
    }

    #[test]
    fn stage1_folder_cell_background_accepts_low_contrast_and_rejects_bad_shape() {
        let settings = default_settings();
        for (value, valid) in [
            (
                json!({"enabled": true, "light": [255,255,230], "dark": [30,35,20]}),
                true,
            ),
            (
                json!({"enabled": false, "light": [255,240,179], "dark": [58,52,32]}),
                true,
            ),
            (
                json!({"enabled": true, "light": [255,244,204], "dark": [220,220,220]}),
                true,
            ),
            (
                json!({"enabled": true, "light": [256,244,204], "dark": [58,52,32]}),
                false,
            ),
            (json!({"enabled": "yes"}), false),
        ] {
            let changes = change("folder_cell_background", value.clone());
            assert_eq!(parse_items(&changes).issues.is_empty(), valid);
            let result = stage1_validate(&settings, &changes);
            assert_eq!(result.is_ok(), valid);
            if valid {
                assert_eq!(
                    serde_json::to_value(result.unwrap().folder_cell_background).unwrap(),
                    value
                );
            }
        }
    }
    #[test]
    fn stage1_marked_option_values_can_reset_to_ui_defaults() {
        let mut settings = default_settings();
        settings.ai_backend = Some("directml".into());
        settings.capture_output_dir = Some(std::env::temp_dir().join("stage1-capture"));
        let changes = Map::from_iter([
            ("ai_backend".into(), Value::Null),
            ("capture_output_dir".into(), Value::Null),
        ]);
        let candidate = stage1_validate(&settings, &changes).unwrap();
        assert_eq!(candidate.ai_backend, None);
        assert_eq!(candidate.capture_output_dir, None);
    }
    #[test]
    fn stage1_rejects_bad_batch_without_mutating_original() {
        let settings = default_settings();
        let original = settings.update_check_enabled;
        let batch = Map::from_iter([
            (
                "parallelism".into(),
                json!({"mode":"Manual","value":PARALLELISM_MAX+1}),
            ),
            ("update_check_enabled".into(), json!(!original)),
        ]);
        assert!(stage1_validate(&settings, &batch).is_err());
        assert_eq!(settings.update_check_enabled, original);
        for (key, value) in [
            ("raw_develop_parallelism", json!(RAW_PARALLELISM_MAX + 1)),
            ("prefetch_back", json!(PREFETCH_MAX + 1)),
            ("gpu_memory_percent", json!(GPU_MEMORY_PERCENT_MAX + 1)),
            ("cache_threshold_ms", json!(CACHE_THRESHOLD_MAX_MS + 1)),
            ("reading_history_limit", json!(0)),
            ("capture_output_dir", json!("relative/path")),
            ("video_volume", json!(10000)),
        ] {
            assert!(
                stage1_validate(&settings, &change(key, value)).is_err(),
                "{key}"
            );
        }
        assert!(
            stage1_validate(
                &settings,
                &change(
                    "file_organize_destinations",
                    json!([{"name":"相対パスの整理先","path":"relative/path"}])
                )
            )
            .is_err()
        );
    }
    #[test]
    fn stage1_refuses_normalization_and_outside_scope_keys() {
        let settings = default_settings();
        assert!(
            stage1_validate(
                &settings,
                &change("details_selection_bar_mode", json!("unknown-value"))
            )
            .is_err()
        );
        for key in [
            "agent_level",
            "agent_show_thumbnails",
            "agent_show_gps",
            "agent_show_ai_captions",
            "agent_show_transcripts",
            "agent_show_current_view",
            "agent_allow_settings",
            "remote_pin",
        ] {
            assert!(
                stage1_validate(&settings, &change(key, Value::Null))
                    .err()
                    .expect("permission keys must be refused")
                    .contains("AI からは変えられません"),
                "{key}"
            );
        }
        let rows = capture_stage1(&settings, false, false);
        for key in [
            "creative_luts",
            "gamepad_enabled",
            "ring_shortcuts",
            "keymap",
            "ui_font",
            "browser_integration_enabled",
            "browser_view_output_dir",
            "show_grid_edge_buttons",
        ] {
            assert!(!rows.iter().any(|row| row.key == key), "{key}");
            let expected = if matches!(key, "keymap" | "ring_shortcuts" | "gamepad_enabled") {
                "set_operations で変えます"
            } else {
                "まだ AI からは変えられません"
            };
            assert!(
                stage1_validate(&settings, &change(key, Value::Null))
                    .err()
                    .expect("stage-2 keys must be refused")
                    .contains(expected),
                "{key}"
            );
        }
    }
    #[test]
    fn stage1_external_tool_template_ids_normalization_and_association() {
        use crate::external_tool::*;
        let mut settings = default_settings();
        let template = external_tools_template();
        assert!(template["example"].get("id").is_none());
        let mut example = template["example"].clone();
        example["name"] = json!("テストツール");
        let candidate = stage1_validate(
            &settings,
            &change("external_tools", json!([example.clone()])),
        )
        .unwrap();
        assert_eq!(candidate.external_tools[0].id, ExternalToolId(1));
        settings.external_tools = candidate.external_tools;
        let candidate = stage1_validate(
            &settings,
            &change(
                "external_tools",
                json!([
                    serde_json::to_value(&settings.external_tools[0]).unwrap(),
                    example.clone()
                ]),
            ),
        )
        .unwrap();
        assert_eq!(candidate.external_tools[1].id, ExternalToolId(2));
        let old = serde_json::to_value(&settings.external_tools[0]).unwrap();
        assert!(
            stage1_validate(
                &settings,
                &change("external_tools", json!([old.clone(), old]))
            )
            .is_err()
        );
        example["payload"] = json!("OriginalFile");
        example["video"] = json!("CurrentFrame");
        let candidate =
            stage1_validate(&settings, &change("external_tools", json!([example]))).unwrap();
        assert_eq!(candidate.external_tools[0].video, VideoPolicy::File);
        assert_eq!(
            candidate.external_tools[0].spread,
            SpreadPolicy::MainPageOnly
        );
        let mut original_bytes = serde_json::to_value(&candidate.external_tools[0]).unwrap();
        original_bytes["payload"] = json!("TempOriginal");
        original_bytes["spread"] = json!("Merged");
        let candidate = stage1_validate(
            &settings,
            &change("external_tools", json!([original_bytes])),
        )
        .unwrap();
        assert_eq!(
            candidate.external_tools[0].spread,
            SpreadPolicy::MainPageOnly
        );
        settings.external_tools[0].launch = ExternalToolLaunch::Association {
            handler_id: "existing-handler".into(),
        };
        let mut association = serde_json::to_value(&settings.external_tools[0]).unwrap();
        association["name"] = json!("名前を変更");
        assert!(
            stage1_validate(
                &settings,
                &change("external_tools", json!([association.clone()]))
            )
            .is_ok()
        );
        association["launch"] = json!({"Association":{"handler_id":"new-handler"}});
        assert!(
            stage1_validate(&settings, &change("external_tools", json!([association]))).is_err()
        );
    }
    #[test]
    fn stage1_external_tool_move_reports_position_key_and_full_command() {
        use crate::external_tool::*;
        let mut settings = default_settings();
        let mut first = ExternalTool::defaults_for_viewing();
        first.name = "最初".into();
        first.launch = ExternalToolLaunch::Executable(Path::new("C:\\Tools\\first.exe").into());
        first.arguments = "--one {files}".into();
        let mut second = first.clone();
        second.id = ExternalToolId(2);
        second.name = "次".into();
        settings.external_tools = vec![first, second];
        let mut after = settings.clone();
        after.external_tools.swap(0, 1);
        let changes = stage1_changes(&settings, &after, &["external_tools".into()]);
        assert_eq!(changes[0].lines.len(), 2);
        let text = format!("{:?}", changes[0].lines);
        for expected in [
            "first.exe",
            "--one {files}",
            "外部ツール 1",
            "外部ツール 2",
            "位置 1",
            "位置 2",
        ] {
            assert!(text.contains(expected), "{expected}");
        }
    }
    #[test]
    fn stage1_bulk_tool_ids_match_next_id_with_reserved_ids_and_u32_wrap() {
        use crate::external_tool::*;
        for (existing_ids, requested_ids) in [
            (vec![1, 3, u32::MAX], vec![None, Some(2), None, None]),
            (vec![u32::MAX - 1], vec![None, None, None]),
            (vec![u32::MAX], vec![None, Some(1), None, Some(3), None]),
            (vec![], vec![None, Some(4), None, Some(2), None]),
        ] {
            let mut settings = default_settings();
            settings.external_tools = existing_ids
                .iter()
                .map(|id| {
                    let mut tool = ExternalTool::defaults_for_viewing();
                    tool.id = ExternalToolId(*id);
                    tool
                })
                .collect();
            let mut reference = settings.external_tools.clone();
            for id in requested_ids.iter().flatten() {
                if !reference.iter().any(|tool| tool.id.0 == *id) {
                    let mut tool = ExternalTool::defaults_for_viewing();
                    tool.id = ExternalToolId(*id);
                    reference.push(tool);
                }
            }
            let mut expected = Vec::new();
            let mut inputs = Vec::new();
            for requested in &requested_ids {
                let mut example = external_tools_template()["example"].clone();
                let id = if let Some(id) = requested {
                    example["id"] = json!(id);
                    ExternalToolId(*id)
                } else {
                    let id = next_id(&reference);
                    let mut tool = ExternalTool::defaults_for_viewing();
                    tool.id = id;
                    reference.push(tool);
                    id
                };
                expected.push(id);
                inputs.push(example);
            }
            let prepared =
                stage1_prepare_changes(&settings, &change("external_tools", json!(inputs)))
                    .unwrap();
            let tools: Vec<ExternalTool> =
                serde_json::from_value(prepared["external_tools"].clone()).unwrap();
            assert_eq!(
                tools.iter().map(|tool| tool.id).collect::<Vec<_>>(),
                expected,
                "existing IDs {existing_ids:?}, request {requested_ids:?}"
            );
        }
        let settings = default_settings();
        let mut duplicate = external_tools_template()["example"].clone();
        duplicate["id"] = json!(u32::MAX);
        assert!(
            stage1_prepare_changes(
                &settings,
                &change("external_tools", json!([duplicate.clone(), duplicate]))
            )
            .unwrap_err()
            .contains("id が重複")
        );
    }
    #[test]
    fn stage1_bulk_tool_formatter_keeps_every_addition_removal_and_move() {
        use crate::external_tool::*;
        let before = (1..=256)
            .map(|id| {
                let mut tool = ExternalTool::defaults_for_viewing();
                tool.id = ExternalToolId(id);
                tool.name = format!("tool-{id}");
                tool
            })
            .collect::<Vec<_>>();
        let mut after = before[1..].to_vec();
        after.reverse();
        let mut added = ExternalTool::defaults_for_viewing();
        added.id = ExternalToolId(257);
        added.name = "added".into();
        after.push(added);
        let lines = external_tool_lines(&before, &after);
        let moved = before
            .iter()
            .enumerate()
            .filter(|(position, old)| {
                after
                    .iter()
                    .position(|tool| tool.id == old.id)
                    .is_some_and(|new_position| *position != new_position)
            })
            .count();
        assert_eq!(lines.len(), moved + 2);
        assert_eq!(
            lines
                .iter()
                .filter(|(item, _, _)| item.contains("を追加"))
                .count(),
            1
        );
        assert_eq!(
            lines
                .iter()
                .filter(|(item, _, _)| item.contains("を削除"))
                .count(),
            1
        );
    }
    #[test]
    fn stage1_vst_state_never_enters_catalog_and_favorite_flags_are_exact() {
        let mut settings = default_settings();
        settings.vst3_plugins.push(Vst3PluginEntry {
            path: "C:\\plugins\\test.vst3".into(),
            state: Some("PRIVATE_PLUGIN_STATE".into()),
            ..Default::default()
        });
        let rows = capture_stage1(&settings, true, false);
        assert!(
            !serde_json::to_string(&rows)
                .unwrap()
                .contains("PRIVATE_PLUGIN_STATE")
        );
        let flagged = rows
            .iter()
            .filter(|row| row.overridden_by_favorite)
            .map(|r| r.key.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(
            flagged,
            HashSet::from([
                "grid_display_order",
                "default_spread_mode",
                "default_reading_flow"
            ])
        );
        assert!(
            capture_stage1(&settings, true, true)
                .iter()
                .find(|r| r.key == "fullscreen_specified_scale")
                .unwrap()
                .overridden_by_favorite
        );
    }
    #[test]
    fn stage1_restored_keys_and_vst_paths_validate_and_preserve_live_runtime_state() {
        let mut settings = default_settings();
        let path = std::env::temp_dir()
            .join("miv-stage1-existing.vst3")
            .to_string_lossy()
            .into_owned();
        let added = std::env::temp_dir()
            .join("miv-stage1-added.vst3")
            .to_string_lossy()
            .into_owned();
        settings.vst3_plugins.push(Vst3PluginEntry {
            path: path.clone(),
            bypass: true,
            user_hidden: true,
            state: Some("PRIVATE_PLUGIN_STATE".into()),
            gui_pos: Some((12, 34)),
            gui_size: Some((640, 480)),
        });
        let rows = capture_stage1(&settings, false, false);
        for key in [
            "susie_enabled",
            "susie_allow_parallel",
            "vst3_enabled",
            "vst3_plugins",
            "ai_backend",
        ] {
            assert!(rows.iter().any(|row| row.key == key), "{key}");
        }
        assert_eq!(
            rows.iter()
                .find(|row| row.key == "vst3_plugins")
                .unwrap()
                .value,
            json!([path])
        );
        let request = Map::from_iter([
            ("susie_enabled".into(), json!(!settings.susie_enabled)),
            (
                "susie_allow_parallel".into(),
                json!(!settings.susie_allow_parallel),
            ),
            ("vst3_enabled".into(), json!(!settings.vst3_enabled)),
            ("vst3_plugins".into(), json!([added, path])),
            ("ai_backend".into(), json!("directml")),
        ]);
        let mut candidate = stage1_validate(&settings, &request).unwrap();
        assert_eq!(
            candidate.vst3_plugins[1].state,
            settings.vst3_plugins[0].state
        );
        assert!(candidate.vst3_plugins[1].bypass);
        assert!(candidate.vst3_plugins[1].user_hidden);
        assert_eq!(candidate.vst3_plugins[1].gui_pos, Some((12, 34)));
        assert_eq!(candidate.vst3_plugins[1].gui_size, Some((640, 480)));
        assert!(candidate.vst3_plugins[0].state.is_none());
        let mut live = settings.clone();
        live.vst3_plugins[0].state = Some("LATEST_PLUGIN_STATE".into());
        let initial_wheel = live.ring_shortcuts.video_normal_wheel_action;
        crate::ui_dialogs::preferences::prepare_preferences_edited_settings_for_commit(
            &mut candidate,
            initial_wheel,
            &mut live,
        );
        assert_eq!(
            candidate.vst3_plugins[1].state.as_deref(),
            Some("LATEST_PLUGIN_STATE")
        );
        let wire = serde_json::to_string(&capture_stage1(&candidate, false, false)).unwrap();
        assert!(!wire.contains("PRIVATE_PLUGIN_STATE"));
        assert!(!wire.contains("LATEST_PLUGIN_STATE"));
        for invalid in [
            json!([path, path]),
            json!(["relative.vst3"]),
            json!([{"path": path, "state":"must not accept runtime state"}]),
            json!(
                (0..=VST3_CHAIN_MAX_LEN)
                    .map(|index| std::env::temp_dir().join(format!("miv-stage1-{index}.vst3")))
                    .collect::<Vec<_>>()
            ),
        ] {
            assert!(stage1_validate(&settings, &change("vst3_plugins", invalid)).is_err());
        }
        let ten = (0..VST3_CHAIN_MAX_LEN)
            .map(|index| std::env::temp_dir().join(format!("miv-stage1-{index}.vst3")))
            .collect::<Vec<_>>();
        assert!(stage1_validate(&settings, &change("vst3_plugins", json!(ten))).is_ok());
        assert!(stage1_validate(&settings, &change("ai_backend", json!("not-a-backend"))).is_err());
    }

    #[test]
    fn stage1_association_deletion_reports_manual_reregistration_and_refuses_ai_restore() {
        use crate::external_tool::*;
        let mut settings = default_settings();
        let mut association = ExternalTool::defaults_for_viewing();
        association.name = "関連付けのテスト".into();
        association.launch = ExternalToolLaunch::Association {
            handler_id: "existing-handler".into(),
        };
        settings.external_tools = vec![association];
        let before = serde_json::to_value(&settings.external_tools).unwrap();
        let after = stage1_validate(&settings, &change("external_tools", json!([]))).unwrap();
        let changes = stage1_changes(&settings, &after, &["external_tools".into()]);
        assert_eq!(changes[0].lines.len(), 1);
        let (item, _, message) = &changes[0].lines[0];
        assert!(item.contains("削除"));
        assert!(message.contains("AI 側からは復元できません"));
        assert!(message.contains("環境設定の画面で登録し直して"));
        assert!(stage1_validate(&after, &change("external_tools", before)).is_err());
    }
    #[test]
    fn stage1_numeric_noop_is_typed_and_unchanged_fields_do_not_reject_migrations() {
        let settings = default_settings();
        let candidate = stage1_validate(&settings, &change("video_volume", json!(1))).unwrap();
        let mut before = settings.clone();
        before.video_volume = 1.0;
        let changes = stage1_changes(&before, &candidate, &["video_volume".into()]);
        assert_eq!(changes[0].before, changes[0].after);
        let mut legacy = settings.clone();
        legacy.gpu_memory_percent = u32::MAX;
        let changed =
            stage1_validate(&legacy, &change("update_check_enabled", json!(true))).unwrap();
        assert_eq!(changed.gpu_memory_percent, u32::MAX);
    }

    #[test]
    fn stage1_page_fit_clears_legacy_margin_fit_carrier() {
        for mode in [FullscreenFitMode::Page, FullscreenFitMode::MarginFit] {
            let mut legacy = default_settings();
            legacy.fullscreen_fit_mode = mode;
            legacy.margin_fit_enabled = true;
            let mut imported = legacy.clone();
            let report =
                parse_items(&change("fullscreen_fit_mode", json!("Page"))).apply_to(&mut imported);
            assert!(report.issues.is_empty());
            assert_eq!(imported.fullscreen_fit_mode, FullscreenFitMode::Page);
            assert!(!imported.margin_fit_enabled);
            let mut candidate =
                stage1_validate(&legacy, &change("fullscreen_fit_mode", json!("Page"))).unwrap();
            assert_eq!(candidate.fullscreen_fit_mode, FullscreenFitMode::Page);
            assert!(!candidate.margin_fit_enabled);
            apply_load_time_migrations(&mut candidate);
            assert_eq!(candidate.fullscreen_fit_mode, FullscreenFitMode::Page);
            assert!(!candidate.margin_fit_enabled);
        }
    }
    #[test]
    fn stage1_marked_edits_survive_the_preferences_live_state_merge() {
        let settings = default_settings();
        let marked = STAGE1_MARKED_ANCHORS
            .iter()
            .map(|(key, _)| *key)
            .collect::<HashSet<_>>();
        let mut exercised = HashSet::new();
        for row in capture_stage1(&settings, false, false) {
            if !marked.contains(row.key.as_str()) {
                continue;
            }
            let mut alternatives = row
                .choices
                .unwrap_or_default()
                .into_iter()
                .map(|choice| choice.value)
                .collect::<Vec<_>>();
            match row.key.as_str() {
                "ai_backend" => alternatives.push(json!("directml")),
                "vst3_plugins" => {
                    alternatives.push(json!([std::env::temp_dir().join("miv-stage1-owner.vst3")]))
                }
                "parallelism" => alternatives.push(json!(Parallelism::Manual(1))),
                "file_organize_destinations" => {
                    alternatives.push(json!([FileOrganizeDestination {
                        name: "所有確認用の整理先".into(),
                        path: std::env::temp_dir().join("miv-stage1-organize-owner"),
                    }]))
                }
                "exif_hidden_tags" => {
                    let mut tags = settings.exif_hidden_tags.clone();
                    tags.push("Stage1OwnershipCustomTag".into());
                    alternatives.push(json!(tags));
                }
                "external_tools" => {
                    let mut tools = settings
                        .external_tools
                        .iter()
                        .map(|tool| serde_json::to_value(tool).unwrap())
                        .collect::<Vec<_>>();
                    let mut example = external_tools_template()["example"].clone();
                    example["name"] = json!("所有確認用の外部ツール");
                    tools.push(example);
                    alternatives.push(json!(tools));
                }
                "menu_layout" => {
                    let mut layout = settings.menu_layout.clone();
                    let command = crate::keymap::MenuCommandId::FileReload
                        .stable_name()
                        .to_string();
                    if layout.hidden_commands.contains(&command) {
                        layout.hidden_commands.retain(|value| value != &command);
                    } else {
                        layout.hidden_commands.push(command);
                    }
                    alternatives.push(json!(layout));
                }
                "context_menu_layout" => {
                    let mut layout = settings.context_menu_layout.clone();
                    let item = crate::context_menu_model::ContextMenuItemId::CutFiles
                        .stable_name()
                        .to_string();
                    if layout.hidden_items.contains(&item) {
                        layout.hidden_items.retain(|value| value != &item);
                    } else {
                        layout.hidden_items.push(item);
                    }
                    alternatives.push(json!(layout));
                }
                "ai_upscale_size_limit" | "ai_denoise_size_limit" => {
                    alternatives.extend([
                        json!(crate::ai::upscale::AiProcessSizeLimit::square(512)),
                        json!(crate::ai::upscale::AiProcessSizeLimit::square(1024)),
                    ]);
                }
                "cache_size_threshold_bytes" => alternatives.extend([
                    json!(CACHE_THRESHOLD_MIN_BYTES),
                    json!(CACHE_THRESHOLD_MAX_BYTES),
                ]),
                "edit_preview_cache_max_bytes" => {
                    alternatives.extend([json!(EDIT_CACHE_MIN_BYTES), json!(EDIT_CACHE_MAX_BYTES)])
                }
                "archive_cache_max_bytes" => alternatives.extend([
                    json!(ARCHIVE_CACHE_MIN_BYTES),
                    json!(ARCHIVE_CACHE_MAX_BYTES),
                ]),
                _ => {}
            }
            match &row.value {
                Value::Bool(value) => alternatives.push(json!(!value)),
                Value::Number(value) => {
                    if let Some(value) = value.as_u64() {
                        alternatives.extend([json!(value.saturating_add(1)), json!(0), json!(1)]);
                    }
                }
                Value::Array(values) => {
                    let mut values = values.clone();
                    values.reverse();
                    alternatives.push(json!(values));
                }
                Value::Null => {
                    alternatives.push(json!(std::env::temp_dir().join("miv-stage1-owned-path")))
                }
                _ => {}
            }
            for alternative in alternatives.into_iter().filter(|value| *value != row.value) {
                if let Ok(mut draft) = stage1_validate(&settings, &change(&row.key, alternative)) {
                    let expected = capture_stage1(&draft, false, false)
                        .into_iter()
                        .find(|r| r.key == row.key)
                        .unwrap()
                        .value;
                    assert_ne!(
                        expected, row.value,
                        "{} did not produce a nondefault edit",
                        row.key
                    );
                    let mut live = settings.clone();
                    draft.overwrite_non_preferences_from(&mut live);
                    let actual = capture_stage1(&draft, false, false)
                        .into_iter()
                        .find(|r| r.key == row.key)
                        .unwrap()
                        .value;
                    assert_eq!(expected, actual, "{}", row.key);
                    exercised.insert(row.key.clone());
                    break;
                }
            }
            assert!(
                exercised.contains(&row.key),
                "{} has no valid nondefault ownership fixture",
                row.key
            );
        }
        assert_eq!(
            exercised.iter().map(String::as_str).collect::<HashSet<_>>(),
            marked
        );
    }
}
