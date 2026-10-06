//! Portable, validated preferences. This module never serializes Settings as a whole.
//! Call the file and JSON functions on a worker; applying a parsed result only edits a draft.
use crate::settings::*;
use serde::{
    Deserialize, Serialize,
    de::{DeserializeOwned, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use std::{
    collections::HashSet,
    fmt,
    fs::File,
    io::{Read, Write},
    path::Path,
};

const FORMAT: &str = "mimageviewer.preferences";
pub const MAX_PREFERENCES_BYTES: usize = 1024 * 1024;
const INVALID_ITEM: &str = "型、選択肢、範囲、または必須項目が正しくありません";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferIssue {
    pub field: String,
    pub reason: String,
}
#[derive(Clone, Debug)]
pub struct ExportedPreferences {
    pub json: String,
    pub issues: Vec<TransferIssue>,
    pub item_count: usize,
}
/// An inexpensive owned capture of permitted values, suitable for sending to a worker.
#[derive(Clone, Debug)]
pub struct ExportSnapshot {
    values: Vec<Accepted>,
    consistent_still: bool,
}
#[derive(Clone, Debug)]
pub struct ParsedPreferences {
    accepted: Vec<Accepted>,
    pub issues: Vec<TransferIssue>,
    pub unknown_count: usize,
}
#[derive(Clone, Debug)]
pub struct ImportReport {
    pub accepted_count: usize,
    pub changed_fields: Vec<String>,
    pub issues: Vec<TransferIssue>,
    pub unknown_count: usize,
}

fn issue(label: &str) -> TransferIssue {
    TransferIssue {
        field: label.into(),
        reason: INVALID_ITEM.into(),
    }
}

// Check the JSON number before f32 rounding can move a value just outside the UI
// limits onto an endpoint. Display gives f32 constants their intended decimal limits.
fn raw_range(raw: &Value, min: impl fmt::Display, max: impl fmt::Display) -> bool {
    raw.as_f64().is_some_and(|value| {
        let min: f64 = min.to_string().parse().expect("numeric minimum");
        let max: f64 = max.to_string().parse().expect("numeric maximum");
        value.is_finite() && (min..=max).contains(&value)
    })
}

fn valid_extensions(values: &Vec<String>, _: &Value) -> bool {
    let expected = default_image_ext_priority();
    let unique: HashSet<_> = values.iter().collect();
    values.len() == expected.len()
        && unique.len() == expected.len()
        && expected.iter().all(|value| unique.contains(value))
}

// Compare only recognized object children, but require every child represented by the
// current type. This bypasses serde(default), unknown-enum and normalization fallbacks.
fn canonical_shape(raw: &Value, canonical: &Value) -> bool {
    match (raw, canonical) {
        (Value::Object(raw), Value::Object(canonical)) => canonical
            .iter()
            .all(|(key, value)| raw.get(key).is_some_and(|raw| canonical_shape(raw, value))),
        (Value::Array(raw), Value::Array(canonical)) => {
            raw.len() == canonical.len()
                && raw
                    .iter()
                    .zip(canonical)
                    .all(|(raw, value)| canonical_shape(raw, value))
        }
        (Value::Number(_), Value::Number(_)) => true,
        _ => raw == canonical,
    }
}

fn validated<T: DeserializeOwned + Serialize>(
    raw: &Value,
    check: impl FnOnce(&T, &Value) -> bool,
) -> Option<T> {
    let value: T = serde_json::from_value(raw.clone()).ok()?;
    let canonical = serde_json::to_value(&value).ok()?;
    (canonical_shape(raw, &canonical) && check(&value, raw)).then_some(value)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum StillLock {
    None,
    BarOnly,
    BarAndStrip,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct StillChrome {
    lock: StillLock,
    visible: bool,
}
fn valid_still(value: &StillChrome, _: &Value) -> bool {
    !matches!(value.lock, StillLock::BarAndStrip) || value.visible
}
fn still_chrome(settings: &Settings) -> StillChrome {
    StillChrome {
        lock: match settings.still_bottom_lock() {
            BottomBarLock::None => StillLock::None,
            BottomBarLock::BarOnly => StillLock::BarOnly,
            BottomBarLock::BarAndStrip => StillLock::BarAndStrip,
        },
        visible: settings.still_seek_strip_visible,
    }
}
fn set_still_chrome(settings: &mut Settings, value: &StillChrome) {
    settings.set_still_bottom_lock(match value.lock {
        StillLock::None => BottomBarLock::None,
        StillLock::BarOnly => BottomBarLock::BarOnly,
        StillLock::BarAndStrip => BottomBarLock::BarAndStrip,
    });
    settings.set_still_seek_strip_visible(value.visible);
}

macro_rules! transfer_get {
    ($s:expr, $f:ident, plain) => {
        $s.$f.clone()
    };
    ($s:expr, $f:ident, archive) => {
        $s.archive_file_handling_resolved()
    };
}
macro_rules! transfer_set {
    ($s:expr, $f:ident, $v:expr, plain) => {
        $s.$f = $v.clone()
    };
    ($s:expr, $f:ident, $v:expr, archive) => {
        $s.set_archive_file_handling(*$v)
    };
    ($s:expr, $f:ident, $v:expr, video_loop) => {
        $s.$f = *$v;
        $s.video_loop = *$v != VideoLoopMode::Off;
    };
}
#[cfg(test)]
macro_rules! assert_excluded_field {
    ($before:expr, $after:expr, favorite_view_overlay) => {
        assert_eq!($before.favorite_view_overlay, $after.favorite_view_overlay);
    };
    ($before:expr, $after:expr, archive_convert_without_dialog) => {};
    ($before:expr, $after:expr, video_loop) => {};
    ($before:expr, $after:expr, $field:ident) => {
        assert_eq!(
            serde_json::to_value(&$before.$field).unwrap(),
            serde_json::to_value(&$after.$field).unwrap(),
            "{}",
            stringify!($field)
        );
    };
}
macro_rules! preferences_policy {
    (export { $( $field:ident : $ty:ty => ($label:literal, $check:expr, $get:ident, $set:ident); )* }
     group { $($group_field:ident),+ }
     exclude { $( $excluded:ident => $reason:literal; )* }) => {
        // No `..`: adding even a serde(skip) or crate-private field requires a policy decision.
        fn exhaustive_policy(settings: &Settings) {
            let Settings { $( $field: _, )* $( $group_field: _, )+ $( $excluded: _, )* } = settings;
        }
        #[allow(non_camel_case_types)]
        #[derive(Clone, Debug)]
        enum Accepted { $( $field($ty), )* StillChrome(StillChrome) }
        pub fn capture_export(settings: &Settings) -> ExportSnapshot {
            exhaustive_policy(settings);
            ExportSnapshot {
                values: vec![$( Accepted::$field(transfer_get!(settings, $field, $get)), )* Accepted::StillChrome(still_chrome(settings))],
                consistent_still: !settings.still_seek_strip_locked || settings.fullscreen_seek_bar_locked,
            }
        }
        fn project(snapshot: ExportSnapshot) -> (Map<String, Value>, Vec<TransferIssue>) {
            let mut map = Map::new(); let mut issues = Vec::new();
            for accepted in snapshot.values { match accepted {
                $( Accepted::$field(value) => match serde_json::to_value(&value) {
                    Ok(raw) if validated::<$ty>(&raw, $check).is_some() => { map.insert(stringify!($field).into(), raw); },
                    _ => issues.push(issue($label)),
                }, )*
                Accepted::StillChrome(value) => {
            // Reject an inconsistent draft rather than silently normalizing it in export.
            if valid_still(&value, &Value::Null) && snapshot.consistent_still {
                map.insert("still_bottom_chrome".into(), serde_json::to_value(value).expect("finite chrome"));
            } else { issues.push(issue("静止画の下部バーとサムネイル列")); }
                },
            }}
            (map, issues)
        }
        fn parse_items(items: &Map<String, Value>) -> ParsedPreferences {
            let mut result = ParsedPreferences { accepted: Vec::new(), issues: Vec::new(), unknown_count: 0 };
            for (key, raw) in items { match key.as_str() {
                $( stringify!($field) => match validated::<$ty>(raw, $check) {
                    Some(value) => result.accepted.push(Accepted::$field(value)),
                    None => result.issues.push(issue($label)),
                }, )*
                "still_bottom_chrome" => match validated::<StillChrome>(raw, valid_still) {
                    Some(value) => result.accepted.push(Accepted::StillChrome(value)),
                    None => result.issues.push(issue("静止画の下部バーとサムネイル列")),
                },
                _ => result.unknown_count += 1,
            }}
            result
        }
        impl ParsedPreferences {
            pub fn apply_to(&self, settings: &mut Settings) -> ImportReport {
                let mut changed_fields = Vec::new();
                for accepted in &self.accepted { match accepted {
                    $( Accepted::$field(value) => {
                        if settings.$field != *value { changed_fields.push($label.into()); }
                        transfer_set!(settings, $field, value, $set);
                    }, )*
                    Accepted::StillChrome(value) => {
                        if still_chrome(settings) != *value || (!settings.fullscreen_seek_bar_locked && settings.still_seek_strip_locked) { changed_fields.push("静止画の下部バーとサムネイル列".into()); }
                        set_still_chrome(settings, value);
                    },
                }}
                ImportReport { accepted_count: self.accepted.len(), changed_fields, issues: self.issues.clone(), unknown_count: self.unknown_count }
            }
        }
        #[cfg(test)]
        fn classifications() -> Vec<(&'static str, Option<&'static str>)> {
            vec![$( (stringify!($field), None), )* $( (stringify!($group_field), None), )+ $( (stringify!($excluded), Some($reason)), )*]
        }
        #[cfg(test)]
        fn wire_keys() -> Vec<&'static str> { vec![$( stringify!($field), )* "still_bottom_chrome"] }
        #[cfg(test)]
        pub(crate) fn assert_excluded_unchanged(before: &Settings, after: &Settings) {
            $( assert_excluded_field!(before, after, $excluded); )*
        }
    }
}

preferences_policy! {
    export {
        ui_theme: UiTheme => ("テーマ", |v, _raw| matches!(v, UiTheme::System | UiTheme::Light | UiTheme::Dark), plain, plain);
        text_contrast: TextContrast => ("文字コントラスト", |v, _raw| matches!(v, TextContrast::Standard | TextContrast::Strong), plain, plain);
        raw_brightness: crate::raw::RawBrightness => ("RAW の明るさ", |v, _raw| matches!(v, crate::raw::RawBrightness::MatchPreview | crate::raw::RawBrightness::None), plain, plain);
        ai_feature_mode: AiFeatureMode => ("AI利用範囲", |_, _| true, plain, plain);
        detached_viewer_open_images_in_window: bool => ("複数ウィンドウモード", |_, _| true, plain, plain);
        auto_fullscreen_zip_pdf: bool => ("本を閲覧表示で開く", |_, _| true, plain, plain);
        auto_fullscreen_image_folders: bool => ("画像フォルダを閲覧表示で開く", |_, _| true, plain, plain);
        fullfeature_media_window: bool => ("動画・音声は別ウィンドウで再生", |_, _| true, plain, plain);
        restore_last_cursor: bool => ("前回のカーソルを復元", |_, _| true, plain, plain);
        startup_window_state: StartupWindowState => ("起動時のウィンドウ状態", |_, _| true, plain, plain);
        grid_click_selection_mode: GridClickSelectionMode => ("サムネイルのクリック選択", |v, _raw| !matches!(v, GridClickSelectionMode::Unknown), plain, plain);
        grid_open_selected_item_on_click: bool => ("選択中の項目をクリックで開く", |_, _| true, plain, plain);
        grid_cursor_wrap: bool => ("サムネイルのカーソル折り返し", |_, _| true, plain, plain);
        remember_favorite_view_state: bool => ("お気に入りごとの表示設定", |_, _| true, plain, plain);
        grid_display_order: GridDisplayOrder => ("カテゴリの表示順", |v, _raw| v == &v.normalized(), plain, plain);
        video_thumbnail_indicator: VideoThumbnailIndicator => ("動画サムネイルの表示", |v, _raw| !matches!(v, VideoThumbnailIndicator::Unknown), plain, plain);
        thumb_show_media_duration: bool => ("サムネイルの再生時間表示", |_, _| true, plain, plain);
        thumb_show_resume_meter: bool => ("サムネイルの読書・再生位置表示", |_, _| true, plain, plain);
        selection_info_display_mode: SelectionInfoDisplayMode => ("選択項目の情報表示", |v, _raw| !matches!(v, SelectionInfoDisplayMode::Unknown), plain, plain);
        thumb_tooltip_show_filename: bool => ("サムネイルツールチップ：ファイル名", |_, _| true, plain, plain);
        thumb_tooltip_show_image_dimensions: bool => ("サムネイルツールチップ：画像サイズ", |_, _| true, plain, plain);
        thumb_tooltip_show_video_duration: bool => ("サムネイルツールチップ：動画の再生時間", |_, _| true, plain, plain);
        thumb_tooltip_show_kind: bool => ("サムネイルツールチップ：種類", |_, _| true, plain, plain);
        thumb_tooltip_show_page_count: bool => ("サムネイルツールチップ：ページ数", |_, _| true, plain, plain);
        thumb_tooltip_show_file_size: bool => ("サムネイルツールチップ：ファイルサイズ", |_, _| true, plain, plain);
        thumb_tooltip_show_modified: bool => ("サムネイルツールチップ：更新日時", |_, _| true, plain, plain);
        thumb_tooltip_show_created: bool => ("サムネイルツールチップ：作成日時", |_, _| true, plain, plain);
        thumb_tooltip_show_video_dimensions: bool => ("サムネイルツールチップ：動画サイズ", |_, _| true, plain, plain);
        thumb_tooltip_show_video_codec: bool => ("サムネイルツールチップ：動画コーデック", |_, _| true, plain, plain);
        thumb_tooltip_show_location: bool => ("サムネイルツールチップ：場所", |_, _| true, plain, plain);
        thumb_tooltip_show_full_location: bool => ("サムネイルツールチップ：完全な場所", |_, _| true, plain, plain);
        thumb_tooltip_show_reading_history_last_read: bool => ("サムネイルツールチップ：最終閲覧日時", |_, _| true, plain, plain);
        thumb_tooltip_show_reading_history_progress: bool => ("サムネイルツールチップ：閲覧進捗", |_, _| true, plain, plain);
        show_windows_context_menu_inline: bool => ("Windowsのコンテキストメニュー", |_, _| true, plain, plain);
        skip_recycle_bin_delete_confirmation: bool => ("ごみ箱への削除確認を省略", |_, _| true, plain, plain);
        rating_sort_unrated_position: crate::rating_sort::RatingSortUnratedPosition => ("未評価項目の位置", |_, _| true, plain, plain);
        slideshow_interval_secs: f32 => ("スライドショーの間隔", |v, _raw| (0.5..=30.0).contains(v) && raw_range(_raw, 0.5, 30.0), plain, plain);
        slideshow_continuous_wait_secs: f32 => ("連結スライドショーの待機時間", |v, _raw| (0.1..=30.0).contains(v) && raw_range(_raw, 0.1, 30.0), plain, plain);
        slideshow_continuous_scroll_secs: f32 => ("連結スライドショーの移動時間", |v, _raw| (0.0..=5.0).contains(v) && raw_range(_raw, 0.0, 5.0), plain, plain);
        slideshow_continuous_scroll_percent: u32 => ("連結スライドショーの移動量", |v, _raw| (1..=100).contains(v), plain, plain);
        slideshow_end_action: SlideshowEndAction => ("スライドショーの終端動作", |_, _| true, plain, plain);
        capture_format: crate::capture::CaptureFormat => ("キャプチャの画像形式", |_, _| true, plain, plain);
        bake_stage_book: crate::bake_stage::BakeStage => ("本への焼き込み", |_, _| true, plain, plain);
        bake_stage_export: crate::bake_stage::BakeStage => ("書き出しの焼き込み", |_, _| true, plain, plain);
        bake_stage_export_batch: crate::bake_stage::BakeStage => ("一括書き出しの焼き込み", |_, _| true, plain, plain);
        bake_stage_external_tool: crate::bake_stage::BakeStage => ("外部ツールへの焼き込み", |_, _| true, plain, plain);
        archive_file_handling: ArchiveFileHandling => ("アーカイブファイルの扱い", |v, _raw| matches!(v, ArchiveFileHandling::Ask | ArchiveFileHandling::Convert | ArchiveFileHandling::Ignore), archive, archive);
        epub_file_handling: EpubFileHandling => ("EPUBファイルの扱い", |_, _| true, plain, plain);
        show_hidden_files: bool => ("隠しファイルを表示", |_, _| true, plain, plain);
        folder_thumb_sort: SortOrder => ("フォルダ代表画像の選定順", |v, _raw| matches!(v, SortOrder::FileName | SortOrder::Numeric | SortOrder::DateAsc | SortOrder::DateDesc), plain, plain);
        folder_thumb_depth: u32 => ("フォルダ代表画像の探索深さ", |v, _raw| (0..=10).contains(v), plain, plain);
        folder_skip_limit: usize => ("画像なしフォルダを飛ばす上限", |v, _raw| (1..=30).contains(v), plain, plain);
        edit_restore_prompt_enabled: bool => ("編集の復元確認", |_, _| true, plain, plain);
        sidecar_backup_enabled: bool => ("編集データのサイドカーバックアップ", |_, _| true, plain, plain);
        tag_sidecar_backup_enabled: bool => ("タグのサイドカーバックアップ", |_, _| true, plain, plain);
        skip_zip_if_folder_exists: bool => ("同名フォルダがあるZIPを省略", |_, _| true, plain, plain);
        skip_archive_if_zip_exists: bool => ("同名ZIPがあるアーカイブを省略", |_, _| true, plain, plain);
        skip_epub_if_pdf_exists: bool => ("同名PDFがあるEPUBを省略", |_, _| true, plain, plain);
        skip_image_if_video_exists: bool => ("同名動画がある画像を省略", |_, _| true, plain, plain);
        skip_duplicate_images: bool => ("同名の重複画像を省略", |_, _| true, plain, plain);
        image_ext_priority: Vec<String> => ("画像拡張子の優先順", valid_extensions, plain, plain);
        minimize_to_tray_on_close: bool => ("閉じるときトレイに常駐", |_, _| true, plain, plain);
        pause_indexer_while_minimized: bool => ("最小化中は索引を停止", |_, _| true, plain, plain);
        write_rating_to_xmp: bool => ("評価をXMPに保存", |_, _| true, plain, plain);
        reading_history_enabled: bool => ("閲覧履歴を記録", |_, _| true, plain, plain);
        default_spread_mode: SpreadMode => ("標準の見開き設定", |v, _| SpreadMode::all().contains(v), plain, plain);
        follow_document_reading_direction: bool => ("文書の綴じ方向に従う", |_, _| true, plain, plain);
        default_reading_flow: ReadingFlow => ("標準の読書フロー", |_, _| true, plain, plain);
        default_reading_direction: ReadingDirection => ("標準の読み方向", |_, _| true, plain, plain);
        final_cover_spread_enabled: bool => ("裏表紙の見開き", |_, _| true, plain, plain);
        singleton_spread_first_enabled: bool => ("最初のページを単独表示", |_, _| true, plain, plain);
        singleton_spread_last_enabled: bool => ("最後のページを単独表示", |_, _| true, plain, plain);
        page_after_cover_alone_enabled: bool => ("表紙の次を単独表示", |_, _| true, plain, plain);
        last_page_alone_enabled: bool => ("最終ページを単独表示", |_, _| true, plain, plain);
        spread_page_gap_px: u32 => ("見開きページ間隔", |v, _raw| (0..=200).contains(v), plain, plain);
        continuous_reading_gap_px: u32 => ("連結読書のページ間隔", |v, _raw| (0..=200).contains(v), plain, plain);
        fullscreen_image_margin_color: [u8; 3] => ("閲覧表示の余白色", |_, _| true, plain, plain);
        fullscreen_fit_mode: FullscreenFitMode => ("閲覧表示のフィット方法", |v, _raw| FullscreenFitMode::all().contains(v), plain, plain);
        fullscreen_fit_no_upscale: bool => ("フィットで拡大しない", |_, _| true, plain, plain);
        fullscreen_fit_no_downscale: bool => ("フィットで縮小しない", |_, _| true, plain, plain);
        fullscreen_side_panel_mode: FsSidePanelMode => ("閲覧表示の左右パネル", |v, _raw| FsSidePanelMode::all().contains(v), plain, plain);
        fullscreen_boundary_notice_visible: bool => ("閲覧表示の境界通知", |_, _| true, plain, plain);
        fullscreen_processing_status_visible: bool => ("閲覧表示の処理状況", |_, _| true, plain, plain);
        fullscreen_prefetch_status_visible: bool => ("閲覧表示の先読み状況", |_, _| true, plain, plain);
        panorama_projection: crate::panorama::PanoProjection => ("パノラマの投影方式", |v, _raw| crate::panorama::PanoProjection::all().contains(v), plain, plain);
        fullscreen_top_bar_locked: bool => ("静止画の上部バー固定", |_, _| true, plain, plain);
        fullscreen_fixed_bar_gap_px: u32 => ("固定バーと画像の間隔", |v, _raw| (0..=FULLSCREEN_FIXED_BAR_GAP_MAX_PX).contains(v), plain, plain);
        fullscreen_seek_direction: FullscreenSeekDirection => ("静止画のシーク方向", |v, _raw| !matches!(v, FullscreenSeekDirection::Unknown), plain, plain);
        fullscreen_horizontal_cursor_direction: FullscreenHorizontalCursorDirection => ("閲覧表示の横カーソル方向", |v, _raw| !matches!(v, FullscreenHorizontalCursorDirection::Unknown), plain, plain);
        fullscreen_page_number_overlay: bool => ("ページ番号を重ねて表示", |_, _| true, plain, plain);
        fullscreen_keep_on_app_switch: bool => ("アプリ切替時に閲覧表示を維持", |_, _| true, plain, plain);
        fullscreen_cursor_hide_delay_secs: f32 => ("閲覧表示のカーソルを隠す時間", |v, _raw| (FULLSCREEN_CURSOR_HIDE_DELAY_MIN_SECS..=FULLSCREEN_CURSOR_HIDE_DELAY_MAX_SECS).contains(v) && raw_range(_raw, FULLSCREEN_CURSOR_HIDE_DELAY_MIN_SECS, FULLSCREEN_CURSOR_HIDE_DELAY_MAX_SECS), plain, plain);
        fullscreen_jump_mode: FullscreenJumpMode => ("ページジャンプの方式", |_, _| true, plain, plain);
        fullscreen_jump_percent: u32 => ("ページジャンプの割合", |v, _raw| (FULLSCREEN_JUMP_PERCENT_MIN..=FULLSCREEN_JUMP_PERCENT_MAX).contains(v), plain, plain);
        fullscreen_fixed_jump_count: usize => ("ページジャンプの枚数", |v, _raw| (FULLSCREEN_FIXED_JUMP_MIN..=FULLSCREEN_FIXED_JUMP_MAX).contains(v), plain, plain);
        continuous_reading_wheel_scroll_percent: u32 => ("連結読書のホイール移動量", |v, _raw| (1..=100).contains(v), plain, plain);
        continuous_reading_key_scroll_percent: u32 => ("連結読書のキー移動量", |v, _raw| (1..=100).contains(v), plain, plain);
        continuous_reading_gamepad_scroll_percent_per_sec: u32 => ("連結読書のゲームパッド移動量", |v, _raw| (10..=300).contains(v), plain, plain);
        still_seek_strip_height: StillSeekStripHeight => ("静止画サムネイル列の高さ", |_, _| true, plain, plain);
        still_seek_strip_height_values: StillSeekStripHeightValues => ("静止画サムネイル列の段階別高さ", |v, _raw| [v.smallest, v.small, v.medium, v.large, v.maximum].iter().all(|n| (STILL_SEEK_STRIP_HEIGHT_MIN_POINTS..=STILL_SEEK_STRIP_HEIGHT_MAX_POINTS).contains(n)), plain, plain);
        still_seek_preview_size: StillSeekPreviewSize => ("静止画シークプレビューの大きさ", |_, _| true, plain, plain);
        still_seek_preview_size_values: StillSeekPreviewSizeValues => ("静止画シークプレビューの段階別高さ", |v, _raw| [v.smallest, v.small, v.medium, v.large, v.maximum].iter().all(|n| (STILL_SEEK_PREVIEW_HEIGHT_MIN_POINTS..=STILL_SEEK_PREVIEW_HEIGHT_MAX_POINTS).contains(n)), plain, plain);
        still_seek_hover_preview_mode: StillSeekHoverPreviewMode => ("静止画シークのホバープレビュー", |_, _| true, plain, plain);
        still_seek_bar_with_strip: StillSeekBarWithStrip => ("静止画サムネイル列とシークバー", |_, _| true, plain, plain);
        video_volume: f64 => ("動画の音量", |v, _raw| (0.0..=VIDEO_VOLUME_MAX).contains(v) && raw_range(_raw, 0.0, VIDEO_VOLUME_MAX), plain, plain);
        video_seek_small_secs: u32 => ("動画の小シーク量", |v, _raw| (VIDEO_SEEK_SECONDS_MIN..=VIDEO_SEEK_SECONDS_MAX).contains(v), plain, plain);
        video_seek_medium_secs: u32 => ("動画の中シーク量", |v, _raw| (VIDEO_SEEK_SECONDS_MIN..=VIDEO_SEEK_SECONDS_MAX).contains(v), plain, plain);
        video_seek_large_secs: u32 => ("動画の大シーク量", |v, _raw| (VIDEO_SEEK_SECONDS_MIN..=VIDEO_SEEK_SECONDS_MAX).contains(v), plain, plain);
        video_seek_thumbnail_tolerance_secs: f64 => ("動画シーク画像の許容時間差", |v, _raw| (VIDEO_SEEK_THUMBNAIL_TOLERANCE_MIN_SECS..=VIDEO_SEEK_THUMBNAIL_TOLERANCE_MAX_SECS).contains(v) && raw_range(_raw, VIDEO_SEEK_THUMBNAIL_TOLERANCE_MIN_SECS, VIDEO_SEEK_THUMBNAIL_TOLERANCE_MAX_SECS), plain, plain);
        video_seek_strip_min_interval_secs: f64 => ("動画サムネイル列の最小間隔", |v, _raw| (VIDEO_SEEK_STRIP_MIN_INTERVAL_MIN_SECS..=VIDEO_SEEK_STRIP_MIN_INTERVAL_MAX_SECS).contains(v) && raw_range(_raw, VIDEO_SEEK_STRIP_MIN_INTERVAL_MIN_SECS, VIDEO_SEEK_STRIP_MIN_INTERVAL_MAX_SECS), plain, plain);
        video_seek_strip_waveform_span_secs: f64 => ("動画波形列の表示時間", |v, _raw| (VIDEO_SEEK_STRIP_WAVEFORM_SPAN_MIN_SECS..=VIDEO_SEEK_STRIP_WAVEFORM_SPAN_MAX_SECS).contains(v) && raw_range(_raw, VIDEO_SEEK_STRIP_WAVEFORM_SPAN_MIN_SECS, VIDEO_SEEK_STRIP_WAVEFORM_SPAN_MAX_SECS), plain, plain);
        video_seek_strip_height: crate::video::seek_strip_layout::SeekStripHeight => ("動画サムネイル列の高さ", |_, _| true, plain, plain);
        video_seek_preview_size: VideoSeekPreviewSize => ("動画シークプレビューの大きさ", |_, _| true, plain, plain);
        video_seek_preview_size_values: VideoSeekPreviewSizeValues => ("動画シークプレビューの段階別幅", |v, _raw| [v.smallest, v.small, v.medium, v.large, v.maximum].iter().all(|n| (VIDEO_SEEK_PREVIEW_WIDTH_MIN_POINTS..=VIDEO_SEEK_PREVIEW_WIDTH_MAX_POINTS).contains(n)), plain, plain);
        video_seek_strip_height_values: crate::video::seek_strip_layout::SeekStripHeightValues => ("動画サムネイル列の段階別高さ", |v, _raw| [v.smallest, v.small, v.medium, v.large, v.maximum].iter().all(|n| (crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MIN_POINTS..=crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MAX_POINTS).contains(n)), plain, plain);
        video_seek_strip_cycle: crate::video::seek_strip_layout::SeekStripCycleSet => ("動画シーク列の巡回候補", |v, _raw| v.thumbnails_window || v.thumbnails_whole || v.waveform_window || v.waveform_whole, plain, plain);
        video_top_bar_locked: bool => ("動画の上部バー固定", |_, _| true, plain, plain);
        video_seek_hover_preview_mode: VideoSeekHoverPreviewMode => ("動画シークのホバープレビュー", |_, _| true, plain, plain);
        video_seek_bar_with_strip: VideoSeekBarWithStrip => ("動画サムネイル列とシークバー", |_, _| true, plain, plain);
        video_loop_mode: VideoLoopMode => ("動画のループ再生", |_, _| true, plain, video_loop);
        video_start_muted: bool => ("動画をミュートで開始", |_, _| true, plain, plain);
        video_thumb_use_sidecar_image: bool => ("動画サムネイルに同名画像を使用", |_, _| true, plain, plain);
        video_grid_open_starts_from_beginning: bool => ("一覧から開く動画の位置復元", |_, _| true, plain, plain);
        video_nav_resume: ResumeMode => ("移動時の動画の位置復元", |_, _| true, plain, plain);
        book_open_resume: ResumeMode => ("本を開くときの位置復元", |_, _| true, plain, plain);
        book_nav_resume: ResumeMode => ("移動時の本の位置復元", |_, _| true, plain, plain);
        music_open_resume: ResumeMode => ("音声を開くときの位置復元", |_, _| true, plain, plain);
        music_nav_resume: ResumeMode => ("移動時の音声の位置復元", |_, _| true, plain, plain);
    }
    group { fullscreen_seek_bar_locked, still_seek_strip_locked, still_seek_strip_visible }
    exclude {
        active_quick_folder_slot => "利用中の A/B ワークスペース。登録先・履歴と一組の利用状態";
        raw_develop_parallelism => "CPU/メモリ/速度に依存する RAW 現像 worker 数の tuning";
        show_facet_sort => "環境設定外のツールバーで管理するソート区画の表示状態";
        toolbar_folder_section_migrated => "ツールバー配置の一度だけの内部移行記録";
        effetune_pre_limiter_enabled => "移行先の EffeTune/VST 導入状態と音声処理構成に依存する微調整";
        effetune_keep_visible_when_minimized => "移行先の EffeTune/VST 導入状態とウィンドウ運用に依存する表示方針";
        grid_cols => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        grid_view_mode => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_sort_key => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_page_count_sort_stash => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_place_sort_stash => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_sort_ascending => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_size_display_mode => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_timestamp_show_seconds => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_row_style => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_column_order => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_column_widths => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_rated_at_width => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_page_count_column_index_stash => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_page_count_column_width_stash => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_place_column_index_stash => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_place_column_width_stash => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_place_column_index_stash => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_place_column_width_stash => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_preview => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_rating => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_rated_at => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_tags => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_kind => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_page_count => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_place => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_size => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_modified => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_created => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_state => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_image_dimensions => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_video_duration => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_video_dimensions => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_show_video_codec => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_name_width_auto => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_name_width => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_mode => "利用者の列構成へ影響するため対象外";
        details_selection_bar_column_order => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_column_widths => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_rated_at_width => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_preview => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_rating => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_rated_at => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_tags => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_kind => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_page_count => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_place => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_size => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_modified => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_created => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_state => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_image_dimensions => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_video_duration => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_video_dimensions => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_show_video_codec => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_name_width_auto => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        details_selection_bar_name_width => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        facet_filter => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        thumb_aspect => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        thumb_aspect_auto => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        favorites => "利用データ・登録先・履歴・検索対象";
        file_organize_destinations => "起動・保存・整理先の PC 固有パス";
        favorite_view_overlay => "runtime overlay / PC のウィンドウ配置";
        smart_folders => "利用データ・登録先・履歴・検索対象";
        last_folder => "利用データ・登録先・履歴・検索対象";
        startup_folder_mode => "起動・保存・整理先の PC 固有パス";
        startup_folder_path => "起動・保存・整理先の PC 固有パス";
        last_cursor_name => "利用データ・登録先・履歴・検索対象";
        last_cursor_rows_above => "利用データ・登録先・履歴・検索対象";
        recent_folders => "利用データ・登録先・履歴・検索対象";
        quick_folder_recent_folders => "利用データ・登録先・履歴・検索対象";
        quick_folder_slots => "利用データ・登録先・履歴・検索対象";
        quick_folder_drive_current_dirs => "利用データ・登録先・履歴・検索対象";
        window_pos => "runtime overlay / PC のウィンドウ配置";
        window_size => "runtime overlay / PC のウィンドウ配置";
        window_maximized => "runtime overlay / PC のウィンドウ配置";
        always_on_top => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        parallelism => "CPU/GPU/メモリ/ドライバ/速度の tuning";
        pdf_worker_count => "CPU/GPU/メモリ/ドライバ/速度の tuning";
        prefetch_back => "CPU/GPU/メモリ/ドライバ/速度の tuning";
        prefetch_forward => "CPU/GPU/メモリ/ドライバ/速度の tuning";
        sort_order => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        rating_view_sort => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        subfolder_expansion_order => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        subfolder_expansion_max_depth => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        subfolder_expansion_filter_kinds => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        subfolder_expansion_filter_date_preset => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        subfolder_expansion_filter_size_preset => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        stack_separator => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        stack_script_enabled => "移行先のローカルスクリプト実行方針を保持";
        thumb_px => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        text_preview_scale => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        text_smart_snap_enabled => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        thumb_quality => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        cache_policy => "キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外";
        cache_threshold_ms => "キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外";
        cache_size_threshold_bytes => "キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外";
        cache_videos_always => "キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外";
        cache_webp_always => "キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外";
        cache_pdf_always => "キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外";
        cache_zip_always => "キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外";
        edit_preview_cache_enabled => "キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外";
        edit_preview_cache_max_bytes => "キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外";
        archive_cache_max_bytes => "キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外";
        archive_convert_without_dialog => "現行 enum の互換 mirror";
        batch_cache_zip_contents => "キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外";
        search_index_checks => "利用データ・登録先・履歴・検索対象";
        indexer_speed_profile => "同上";
        batch_cache_pdf_contents => "キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外";
        thumb_prev_pages => "CPU/GPU/メモリ/ドライバ/速度の tuning";
        thumb_next_pages => "CPU/GPU/メモリ/ドライバ/速度の tuning";
        gpu_memory_percent => "CPU/GPU/メモリ/ドライバ/速度の tuning";
        thumb_idle_upgrade => "同上";
        tags => "利用データ・登録先・履歴・検索対象";
        show_toolbar_favorites => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_smart_folders => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_tags => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        folder_tree_pane_visible => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        folder_tree_sort_order => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        folder_tree_pane_width_ratio => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_folder => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_folder_tree_button => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_effetune => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_bookshelf => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_collections => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_address_bar_history_nav => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_address_bar_quick_folders => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_parent_button => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_prev_folder => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_next_folder => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_vst3 => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_rating => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_facet_filter => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_address_bar_favorite_button => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_address_bar_history_menu => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_address_bar_folder_pin => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_address_bar_stack_toggle => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_address_bar_omitted_entries => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_location_drive_list => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_location_reading_history => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_location_rating => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_location_bookshelf => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_location_file_organize_destinations => "場所▼の表示状態。他の show_location_* と同じく転送対象外";
        show_location_desktop => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_location_pictures => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_location_downloads => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_location_drive_roots => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        use_native_shell_context_menu => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        ring_shortcuts => "既存の操作カスタマイズ共有が正本";
        rating_filter => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        exif_hidden_tags => "保持数の prune、EXIF 任意文字列、動画下部固定の scope 外状態変更、デインターレースの性能 tuning";
        capture_output_dir => "起動・保存・整理先の PC 固有パス";
        book_root => "起動・保存・整理先の PC 固有パス";
        active_book_name => "利用データ・登録先・履歴・検索対象";
        pinned_books => "同上";
        conceal_type => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        conceal_mosaic_tile_mode => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        conceal_mosaic_boundary => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        conceal_fill_opacity_percent => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        conceal_fill_edge => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        conceal_blur_radius_px => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        conceal_blur_mode => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        conceal_blur_feather => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        conceal_brush_radius => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        conceal_line_width => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        conceal_presets => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        export_embed_metadata => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        export_last_directory => "起動・保存・整理先の PC 固有パス";
        export_fallback_format => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        export_default_scale => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        export_batch_selection => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        export_batch_directory => "起動・保存・整理先の PC 固有パス";
        export_batch_template => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        export_batch_format => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        export_batch_scale => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        sns_split_target => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        sns_split_count => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        sns_split_seam_permille => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        sns_split_frame_ratio => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        downscale_smoothing_percent => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        anime_upscale_source_limit => "同上";
        fullscreen_left_panel_tab => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        adjustment_settings_tab => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        creative_luts => "フォント / ツール / LUT / Susie / VST の実ファイル・導入状態・任意 state に依存";
        fullscreen_navigator_visible => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        fullscreen_navigator_corner => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        fullscreen_navigator_size => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        touch_still_chrome_learned => "初回/学習/通知/保存版の内部記録";
        touch_video_chrome_learned => "初回/学習/通知/保存版の内部記録";
        gamepad_enabled => "既存の操作カスタマイズ共有が正本";
        margin_fit_enabled => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        ui_scale_factor => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        ui_font => "フォント / ツール / LUT / Susie / VST の実ファイル・導入状態・任意 state に依存";
        first_setup_completed => "初回/学習/通知/保存版の内部記録";
        toolbar_cols_items => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_cols_20_options_migrated => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_cols_details_visible => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_aspect_items => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_aspect_auto_visible => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_cols_display => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_aspect_display => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_sort_display => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_favorites_display => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_smart_folders_display => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_tags_display => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_bookshelf_display => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_collections_display => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_collection_target_id => "同上";
        pinned_collections => "同上";
        toolbar_favorites_collapsed => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_smart_folders_collapsed => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_tags_collapsed => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_bookshelf_collapsed => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_collections_collapsed => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_sort_items => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_sort_size_options_migrated => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_sort_name_numeric_desc_options_migrated => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_sort_rating_options_migrated => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_facet_filter_items => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_facet_name_filter_index_stash => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        facet_name_filter_width => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_section_order => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_cols => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_aspect => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        show_toolbar_sort => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_section_new_row => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        toolbar_section_drag_enabled => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        menu_layout => "既存の操作カスタマイズ共有が正本";
        context_menu_layout => "既存の操作カスタマイズ共有が正本";
        keymap => "既存の操作カスタマイズ共有が正本";
        recent_open_with_apps => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        custom_open_with_apps => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        external_tools => "フォント / ツール / LUT / Susie / VST の実ファイル・導入状態・任意 state に依存";
        ai_upscale_enabled => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        ai_upscale_model_override => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        ai_upscale_prefetch_back => "CPU/GPU/メモリ/ドライバ/速度の tuning";
        ai_upscale_prefetch_forward => "CPU/GPU/メモリ/ドライバ/速度の tuning";
        retained_final_ai_cache_max_entries => "CPU/GPU/メモリ/ドライバ/速度の tuning";
        retained_final_ai_cache_max_mib => "CPU/GPU/メモリ/ドライバ/速度の tuning";
        ai_upscale_skip_px => "CPU/GPU/メモリ/ドライバ/速度の tuning";
        ai_denoise_skip_px => "同上";
        ai_upscale_size_limit => "同上";
        ai_denoise_size_limit => "同上";
        ai_backend => "同上";
        erase_inpaint_mono_tolerance => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        global_preset => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        preset_slots => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        post_filter_global_preset_stash => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        post_filter_preset_slot_stashes => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        colorize_preset_slots => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        metadata_export_recursive => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        susie_enabled => "フォント / ツール / LUT / Susie / VST の実ファイル・導入状態・任意 state に依存";
        susie_allow_parallel => "フォント / ツール / LUT / Susie / VST の実ファイル・導入状態・任意 state に依存";
        skip_offline_change_scan => "同上";
        update_check_enabled => "接続/送出・ローカル出力方針、自動通信の opt-in は移行先を維持";
        update_check_dismissed_version => "初回/学習/通知/保存版の内部記録";
        network_data_dir_notice_dismissed_for => "初回/学習/通知/保存版の内部記録";
        perf_log_enabled => "初回/学習/通知/保存版の内部記録";
        remote_service_enabled => "接続/送出・ローカル出力方針、自動通信の opt-in は移行先を維持";
        remote_video_streaming_enabled => "接続/送出・ローカル出力方針、自動通信の opt-in は移行先を維持";
        remote_video_encoder => "接続/送出・ローカル出力方針、自動通信の opt-in は移行先を維持";
        remote_video_quality_default => "接続/送出・ローカル出力方針、自動通信の opt-in は移行先を維持";
        remote_video_segment_window => "接続/送出・ローカル出力方針、自動通信の opt-in は移行先を維持";
        remote_video_mute_local_output => "接続/送出・ローカル出力方針、自動通信の opt-in は移行先を維持";
        remote_video_hide_local_output => "接続/送出・ローカル出力方針、自動通信の opt-in は移行先を維持";
        video_playback_speed => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_seek_strip_state => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_seek_strip_last_choice => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_seek_strip_span => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_seek_bar_locked => "保持数の prune、EXIF 任意文字列、動画下部固定の scope 外状態変更、デインターレースの性能 tuning";
        video_seek_strip_locked => "保持数の prune、EXIF 任意文字列、動画下部固定の scope 外状態変更、デインターレースの性能 tuning";
        video_autoplay => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_autoplay_mode => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_loop => "現行 enum の互換 mirror";
        video_continuous_mode => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_muted => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_adjustments => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_scale_filter => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_downscale_smoothing_percent => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_anime4k_budget => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_anime4k_measurement => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_preset_slots => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_resume_positions => "同上";
        video_watched_to_end => "同上";
        video_audio_track_choices => "同上";
        reading_history_limit => "保持数の prune、EXIF 任意文字列、動画下部固定の scope 外状態変更、デインターレースの性能 tuning";
        video_hw_decode => "同上";
        video_deinterlace => "保持数の prune、EXIF 任意文字列、動画下部固定の scope 外状態変更、デインターレースの性能 tuning";
        video_tile_columns => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        video_in_window_mode => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        detached_viewer_enabled => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        detached_viewer_window_placement => "runtime overlay / PC のウィンドウ配置";
        vst3_enabled => "フォント / ツール / LUT / Susie / VST の実ファイル・導入状態・任意 state に依存";
        vst3_plugins => "フォント / ツール / LUT / Susie / VST の実ファイル・導入状態・任意 state に依存";
        vst3_plugin_path => "フォント / ツール / LUT / Susie / VST の実ファイル・導入状態・任意 state に依存";
        vst3_plugin_state => "フォント / ツール / LUT / Susie / VST の実ファイル・導入状態・任意 state に依存";
        vst3_gui_visible => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        effetune_gui_pos => "runtime overlay / PC のウィンドウ配置";
        effetune_gui_size => "runtime overlay / PC のウィンドウ配置";
        vst3_video_compact => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        vst3_panel_pos => "runtime overlay / PC のウィンドウ配置";
        vst3_chain_slots => "フォント / ツール / LUT / Susie / VST の実ファイル・導入状態・任意 state に依存";
        audio_normalize_enabled => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        audio_normalize_target_lufs_milli => "環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier";
        last_seen_version => "初回/学習/通知/保存版の内部記録";
    }
}

/// Serialize only validated transfer fields; excluded values are never inspected.
pub fn export_preferences(settings: &Settings) -> Result<ExportedPreferences, String> {
    capture_export(settings).export()
}

impl ExportSnapshot {
    pub fn export(self) -> Result<ExportedPreferences, String> {
        let (preferences, issues) = project(self);
        let item_count = preferences.len();
        let document = serde_json::json!({ "format": FORMAT, "format_version": 1, "app_version": env!("CARGO_PKG_VERSION"), "preferences": preferences });
        let mut json = serde_json::to_string_pretty(&document)
            .map_err(|_| "設定ファイルを作成できませんでした".to_string())?;
        json.push('\n');
        Ok(ExportedPreferences {
            json,
            issues,
            item_count,
        })
    }
}

// A recursive Value visitor keeps serde_json's normal depth limit and rejects duplicate
// keys even in ignored/unknown objects. Errors deliberately omit untrusted input text.
struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON value without duplicate keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| UniqueValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(UniqueValue(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some(key) = object.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                    let UniqueValue(value) = object.next_value()?;
                    values.insert(key, value);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

pub fn parse_preferences(json: &str) -> Result<ParsedPreferences, String> {
    if json.len() > MAX_PREFERENCES_BYTES {
        return Err("設定ファイルは 1 MiB 以下にしてください".into());
    }
    let UniqueValue(document) = serde_json::from_str(json.strip_prefix('\u{feff}').unwrap_or(json))
        .map_err(|_| {
            "設定ファイルの JSON が正しくありません（重複キーや深さ超過を含みます）".to_string()
        })?;
    let object = document
        .as_object()
        .ok_or("設定ファイルの形式が正しくありません")?;
    if object.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err("環境設定の持ち運びファイルではありません".into());
    }
    if object.get("format_version").and_then(Value::as_u64) != Some(1) {
        return Err("対応していない設定ファイル形式の版です".into());
    }
    let preferences = object
        .get("preferences")
        .and_then(Value::as_object)
        .ok_or("設定項目の形式が正しくありません")?;
    Ok(parse_items(preferences))
}

pub fn read_preferences(path: &Path) -> Result<ParsedPreferences, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| {
            file.take((MAX_PREFERENCES_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|_| "設定ファイルを読み込めませんでした".to_string())?;
    if bytes.len() > MAX_PREFERENCES_BYTES {
        return Err("設定ファイルは 1 MiB 以下にしてください".into());
    }
    let json = std::str::from_utf8(&bytes)
        .map_err(|_| "設定ファイルは UTF-8 にしてください".to_string())?;
    parse_preferences(json)
}

pub fn write_preferences(path: &Path, json: &str) -> Result<(), String> {
    // tempfile owns cleanup, including failed publication. It is always a sibling;
    // the existing destination is never opened with truncate.
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::Builder::new()
        .prefix(".mivprefs-")
        .tempfile_in(parent)
        .map_err(|_| "設定ファイルの一時ファイルを作成できませんでした".to_string())?;
    temp.write_all(json.as_bytes())
        .and_then(|_| temp.flush())
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| "設定ファイルを書き込めませんでした".to_string())?;
    let (file, temp_path) = temp.into_parts();
    drop(file);
    crate::archive_converter::replace_file_atomic(&temp_path, path, false)
        .map_err(|_| "設定ファイルを保存できませんでした".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Preserve this first-version fixture when adding v1 keys in future releases.
    const INITIAL_V1: &str = r#"{"format":"mimageviewer.preferences","format_version":1,"app_version":"4.3.0","preferences":{"ui_theme":"Dark","show_hidden_files":true,"video_seek_small_secs":3,"still_bottom_chrome":{"lock":"BarOnly","visible":true}}}"#;

    fn document(items: Value) -> String {
        json!({"format": FORMAT, "format_version": 1, "preferences": items}).to_string()
    }
    fn different_enum<T: Clone + Serialize>(current: &T, choices: &[T]) -> T {
        choices
            .iter()
            .find(|value| {
                serde_json::to_value(value).unwrap() != serde_json::to_value(current).unwrap()
            })
            .expect("multiple current UI choices")
            .clone()
    }
    fn nondefault_source() -> Settings {
        let mut settings = Settings::default();
        settings.raw_brightness = crate::raw::RawBrightness::None;
        settings.ui_theme = different_enum(
            &settings.ui_theme,
            &[UiTheme::System, UiTheme::Light, UiTheme::Dark],
        );
        settings.text_contrast = different_enum(
            &settings.text_contrast,
            &[TextContrast::Standard, TextContrast::Strong],
        );
        settings.ai_feature_mode = different_enum(
            &settings.ai_feature_mode,
            &[
                AiFeatureMode::Disabled,
                AiFeatureMode::Light,
                AiFeatureMode::HighQuality,
            ],
        );
        settings.detached_viewer_open_images_in_window =
            !settings.detached_viewer_open_images_in_window;
        settings.auto_fullscreen_zip_pdf = !settings.auto_fullscreen_zip_pdf;
        settings.auto_fullscreen_image_folders = !settings.auto_fullscreen_image_folders;
        settings.fullfeature_media_window = !settings.fullfeature_media_window;
        settings.restore_last_cursor = !settings.restore_last_cursor;
        settings.startup_window_state = different_enum(
            &settings.startup_window_state,
            &[
                StartupWindowState::RememberLast,
                StartupWindowState::Normal,
                StartupWindowState::Maximized,
            ],
        );
        settings.grid_click_selection_mode = different_enum(
            &settings.grid_click_selection_mode,
            &[
                GridClickSelectionMode::Check,
                GridClickSelectionMode::Explorer,
            ],
        );
        settings.grid_open_selected_item_on_click = !settings.grid_open_selected_item_on_click;
        settings.grid_cursor_wrap = !settings.grid_cursor_wrap;
        settings.remember_favorite_view_state = !settings.remember_favorite_view_state;
        settings.grid_display_order = GridDisplayOrder::from_rows([
            vec![],
            vec![],
            vec![GridItemDisplayKind::VideoAudio, GridItemDisplayKind::Image],
            vec![GridItemDisplayKind::Archive, GridItemDisplayKind::Folder],
        ]);
        settings.video_thumbnail_indicator = different_enum(
            &settings.video_thumbnail_indicator,
            &[
                VideoThumbnailIndicator::PlayIcon,
                VideoThumbnailIndicator::BottomLeftBadge,
                VideoThumbnailIndicator::Hidden,
            ],
        );
        settings.thumb_show_media_duration = !settings.thumb_show_media_duration;
        settings.thumb_show_resume_meter = !settings.thumb_show_resume_meter;
        settings.selection_info_display_mode = different_enum(
            &settings.selection_info_display_mode,
            &[
                SelectionInfoDisplayMode::Tooltip,
                SelectionInfoDisplayMode::BottomBar,
                SelectionInfoDisplayMode::Both,
                SelectionInfoDisplayMode::Hidden,
            ],
        );
        settings.thumb_tooltip_show_filename = !settings.thumb_tooltip_show_filename;
        settings.thumb_tooltip_show_image_dimensions =
            !settings.thumb_tooltip_show_image_dimensions;
        settings.thumb_tooltip_show_video_duration = !settings.thumb_tooltip_show_video_duration;
        settings.thumb_tooltip_show_kind = !settings.thumb_tooltip_show_kind;
        settings.thumb_tooltip_show_page_count = !settings.thumb_tooltip_show_page_count;
        settings.thumb_tooltip_show_file_size = !settings.thumb_tooltip_show_file_size;
        settings.thumb_tooltip_show_modified = !settings.thumb_tooltip_show_modified;
        settings.thumb_tooltip_show_created = !settings.thumb_tooltip_show_created;
        settings.thumb_tooltip_show_video_dimensions =
            !settings.thumb_tooltip_show_video_dimensions;
        settings.thumb_tooltip_show_video_codec = !settings.thumb_tooltip_show_video_codec;
        settings.thumb_tooltip_show_location = !settings.thumb_tooltip_show_location;
        settings.thumb_tooltip_show_full_location = !settings.thumb_tooltip_show_full_location;
        settings.thumb_tooltip_show_reading_history_last_read =
            !settings.thumb_tooltip_show_reading_history_last_read;
        settings.thumb_tooltip_show_reading_history_progress =
            !settings.thumb_tooltip_show_reading_history_progress;
        settings.show_windows_context_menu_inline = !settings.show_windows_context_menu_inline;
        settings.skip_recycle_bin_delete_confirmation =
            !settings.skip_recycle_bin_delete_confirmation;
        settings.rating_sort_unrated_position = different_enum(
            &settings.rating_sort_unrated_position,
            &[
                crate::rating_sort::RatingSortUnratedPosition::BetweenThreeAndTwo,
                crate::rating_sort::RatingSortUnratedPosition::BelowAll,
            ],
        );
        settings.slideshow_interval_secs = if settings.slideshow_interval_secs == 30.0 {
            0.5
        } else {
            30.0
        };
        settings.slideshow_continuous_wait_secs = if settings.slideshow_continuous_wait_secs == 30.0
        {
            0.1
        } else {
            30.0
        };
        settings.slideshow_continuous_scroll_secs =
            if settings.slideshow_continuous_scroll_secs == 5.0 {
                0.0
            } else {
                5.0
            };
        settings.slideshow_continuous_scroll_percent =
            if settings.slideshow_continuous_scroll_percent == 100 {
                1
            } else {
                100
            };
        settings.slideshow_end_action = different_enum(
            &settings.slideshow_end_action,
            &[
                SlideshowEndAction::LoopFolder,
                SlideshowEndAction::NextFolder,
                SlideshowEndAction::Stop,
            ],
        );
        settings.capture_format = different_enum(
            &settings.capture_format,
            &[
                crate::capture::CaptureFormat::Png,
                crate::capture::CaptureFormat::Jpeg95,
                crate::capture::CaptureFormat::Jpeg85,
                crate::capture::CaptureFormat::Jpeg75,
            ],
        );
        settings.bake_stage_book = different_enum(
            &settings.bake_stage_book,
            &[
                crate::bake_stage::BakeStage::Edits,
                crate::bake_stage::BakeStage::Ai,
                crate::bake_stage::BakeStage::DisplayAdjust,
            ],
        );
        settings.bake_stage_export = different_enum(
            &settings.bake_stage_export,
            &[
                crate::bake_stage::BakeStage::Edits,
                crate::bake_stage::BakeStage::Ai,
                crate::bake_stage::BakeStage::DisplayAdjust,
            ],
        );
        settings.bake_stage_export_batch = different_enum(
            &settings.bake_stage_export_batch,
            &[
                crate::bake_stage::BakeStage::Edits,
                crate::bake_stage::BakeStage::Ai,
                crate::bake_stage::BakeStage::DisplayAdjust,
            ],
        );
        settings.bake_stage_external_tool = different_enum(
            &settings.bake_stage_external_tool,
            &[
                crate::bake_stage::BakeStage::Edits,
                crate::bake_stage::BakeStage::Ai,
                crate::bake_stage::BakeStage::DisplayAdjust,
            ],
        );
        settings.archive_file_handling = different_enum(
            &settings.archive_file_handling,
            &[
                ArchiveFileHandling::Ask,
                ArchiveFileHandling::Convert,
                ArchiveFileHandling::Ignore,
            ],
        );
        settings.epub_file_handling = different_enum(
            &settings.epub_file_handling,
            &[
                EpubFileHandling::Ask,
                EpubFileHandling::Convert,
                EpubFileHandling::Ignore,
            ],
        );
        settings.show_hidden_files = !settings.show_hidden_files;
        settings.folder_thumb_sort = different_enum(
            &settings.folder_thumb_sort,
            &[
                SortOrder::FileName,
                SortOrder::Numeric,
                SortOrder::DateAsc,
                SortOrder::DateDesc,
            ],
        );
        settings.folder_thumb_depth = if settings.folder_thumb_depth == 10 {
            0
        } else {
            10
        };
        settings.folder_skip_limit = if settings.folder_skip_limit == 30 {
            1
        } else {
            30
        };
        settings.edit_restore_prompt_enabled = !settings.edit_restore_prompt_enabled;
        settings.sidecar_backup_enabled = !settings.sidecar_backup_enabled;
        settings.tag_sidecar_backup_enabled = !settings.tag_sidecar_backup_enabled;
        settings.skip_zip_if_folder_exists = !settings.skip_zip_if_folder_exists;
        settings.skip_archive_if_zip_exists = !settings.skip_archive_if_zip_exists;
        settings.skip_epub_if_pdf_exists = !settings.skip_epub_if_pdf_exists;
        settings.skip_image_if_video_exists = !settings.skip_image_if_video_exists;
        settings.skip_duplicate_images = !settings.skip_duplicate_images;
        settings.image_ext_priority.reverse();
        settings.minimize_to_tray_on_close = !settings.minimize_to_tray_on_close;
        settings.pause_indexer_while_minimized = !settings.pause_indexer_while_minimized;
        settings.write_rating_to_xmp = !settings.write_rating_to_xmp;
        settings.reading_history_enabled = !settings.reading_history_enabled;
        settings.default_spread_mode = different_enum(
            &settings.default_spread_mode,
            &[
                SpreadMode::Single,
                SpreadMode::Ltr,
                SpreadMode::LtrCover,
                SpreadMode::Rtl,
                SpreadMode::RtlCover,
                SpreadMode::SplitLtr,
                SpreadMode::SplitRtl,
            ],
        );
        settings.follow_document_reading_direction = !settings.follow_document_reading_direction;
        settings.default_reading_flow = different_enum(
            &settings.default_reading_flow,
            &[
                ReadingFlow::Paged,
                ReadingFlow::Vertical,
                ReadingFlow::Horizontal,
            ],
        );
        settings.default_reading_direction = different_enum(
            &settings.default_reading_direction,
            &[ReadingDirection::Ltr, ReadingDirection::Rtl],
        );
        settings.final_cover_spread_enabled = !settings.final_cover_spread_enabled;
        settings.singleton_spread_first_enabled = !settings.singleton_spread_first_enabled;
        settings.singleton_spread_last_enabled = !settings.singleton_spread_last_enabled;
        settings.page_after_cover_alone_enabled = !settings.page_after_cover_alone_enabled;
        settings.last_page_alone_enabled = !settings.last_page_alone_enabled;
        settings.spread_page_gap_px = if settings.spread_page_gap_px == 200 {
            0
        } else {
            200
        };
        settings.continuous_reading_gap_px = if settings.continuous_reading_gap_px == 200 {
            0
        } else {
            200
        };
        settings.fullscreen_image_margin_color = [23, 49, 91];
        settings.fullscreen_fit_mode = different_enum(
            &settings.fullscreen_fit_mode,
            &[
                FullscreenFitMode::Page,
                FullscreenFitMode::Width,
                FullscreenFitMode::Height,
                FullscreenFitMode::Original,
            ],
        );
        settings.fullscreen_fit_no_upscale = !settings.fullscreen_fit_no_upscale;
        settings.fullscreen_fit_no_downscale = !settings.fullscreen_fit_no_downscale;
        settings.fullscreen_side_panel_mode = different_enum(
            &settings.fullscreen_side_panel_mode,
            &[FsSidePanelMode::Hover, FsSidePanelMode::ClickToShow],
        );
        settings.fullscreen_boundary_notice_visible = !settings.fullscreen_boundary_notice_visible;
        settings.fullscreen_processing_status_visible =
            !settings.fullscreen_processing_status_visible;
        settings.fullscreen_prefetch_status_visible = !settings.fullscreen_prefetch_status_visible;
        settings.panorama_projection = different_enum(
            &settings.panorama_projection,
            &[
                crate::panorama::PanoProjection::Perspective,
                crate::panorama::PanoProjection::Stereographic,
                crate::panorama::PanoProjection::Equidistant,
                crate::panorama::PanoProjection::EquisolidAngle,
            ],
        );
        settings.fullscreen_top_bar_locked = !settings.fullscreen_top_bar_locked;
        settings.fullscreen_fixed_bar_gap_px =
            if settings.fullscreen_fixed_bar_gap_px == FULLSCREEN_FIXED_BAR_GAP_MAX_PX {
                0
            } else {
                FULLSCREEN_FIXED_BAR_GAP_MAX_PX
            };
        settings.fullscreen_seek_direction = different_enum(
            &settings.fullscreen_seek_direction,
            &[
                FullscreenSeekDirection::FollowReading,
                FullscreenSeekDirection::LeftToRight,
            ],
        );
        settings.fullscreen_horizontal_cursor_direction = different_enum(
            &settings.fullscreen_horizontal_cursor_direction,
            &[
                FullscreenHorizontalCursorDirection::FollowPage,
                FullscreenHorizontalCursorDirection::FollowSeekBar,
            ],
        );
        settings.fullscreen_page_number_overlay = !settings.fullscreen_page_number_overlay;
        settings.fullscreen_keep_on_app_switch = !settings.fullscreen_keep_on_app_switch;
        settings.fullscreen_cursor_hide_delay_secs = if settings.fullscreen_cursor_hide_delay_secs
            == FULLSCREEN_CURSOR_HIDE_DELAY_MAX_SECS
        {
            FULLSCREEN_CURSOR_HIDE_DELAY_MIN_SECS
        } else {
            FULLSCREEN_CURSOR_HIDE_DELAY_MAX_SECS
        };
        settings.fullscreen_jump_mode = different_enum(
            &settings.fullscreen_jump_mode,
            &[FullscreenJumpMode::Percent, FullscreenJumpMode::FixedPages],
        );
        settings.fullscreen_jump_percent =
            if settings.fullscreen_jump_percent == FULLSCREEN_JUMP_PERCENT_MAX {
                FULLSCREEN_JUMP_PERCENT_MIN
            } else {
                FULLSCREEN_JUMP_PERCENT_MAX
            };
        settings.fullscreen_fixed_jump_count =
            if settings.fullscreen_fixed_jump_count == FULLSCREEN_FIXED_JUMP_MAX {
                FULLSCREEN_FIXED_JUMP_MIN
            } else {
                FULLSCREEN_FIXED_JUMP_MAX
            };
        settings.continuous_reading_wheel_scroll_percent =
            if settings.continuous_reading_wheel_scroll_percent == 100 {
                1
            } else {
                100
            };
        settings.continuous_reading_key_scroll_percent =
            if settings.continuous_reading_key_scroll_percent == 100 {
                1
            } else {
                100
            };
        settings.continuous_reading_gamepad_scroll_percent_per_sec =
            if settings.continuous_reading_gamepad_scroll_percent_per_sec == 300 {
                10
            } else {
                300
            };
        settings.still_seek_strip_height = different_enum(
            &settings.still_seek_strip_height,
            &[
                StillSeekStripHeight::Maximum,
                StillSeekStripHeight::Large,
                StillSeekStripHeight::Medium,
                StillSeekStripHeight::Small,
                StillSeekStripHeight::Smallest,
            ],
        );
        settings.still_seek_strip_height_values.smallest =
            if settings.still_seek_strip_height_values.smallest
                == STILL_SEEK_STRIP_HEIGHT_MAX_POINTS
            {
                STILL_SEEK_STRIP_HEIGHT_MIN_POINTS
            } else {
                STILL_SEEK_STRIP_HEIGHT_MAX_POINTS
            };
        settings.still_seek_strip_height_values.small = if settings
            .still_seek_strip_height_values
            .small
            == STILL_SEEK_STRIP_HEIGHT_MAX_POINTS
        {
            STILL_SEEK_STRIP_HEIGHT_MIN_POINTS
        } else {
            STILL_SEEK_STRIP_HEIGHT_MAX_POINTS
        };
        settings.still_seek_strip_height_values.medium =
            if settings.still_seek_strip_height_values.medium == STILL_SEEK_STRIP_HEIGHT_MAX_POINTS
            {
                STILL_SEEK_STRIP_HEIGHT_MIN_POINTS
            } else {
                STILL_SEEK_STRIP_HEIGHT_MAX_POINTS
            };
        settings.still_seek_strip_height_values.large = if settings
            .still_seek_strip_height_values
            .large
            == STILL_SEEK_STRIP_HEIGHT_MAX_POINTS
        {
            STILL_SEEK_STRIP_HEIGHT_MIN_POINTS
        } else {
            STILL_SEEK_STRIP_HEIGHT_MAX_POINTS
        };
        settings.still_seek_strip_height_values.maximum = if settings
            .still_seek_strip_height_values
            .maximum
            == STILL_SEEK_STRIP_HEIGHT_MAX_POINTS
        {
            STILL_SEEK_STRIP_HEIGHT_MIN_POINTS
        } else {
            STILL_SEEK_STRIP_HEIGHT_MAX_POINTS
        };
        settings.still_seek_preview_size = different_enum(
            &settings.still_seek_preview_size,
            &[
                StillSeekPreviewSize::Maximum,
                StillSeekPreviewSize::Large,
                StillSeekPreviewSize::Medium,
                StillSeekPreviewSize::Small,
                StillSeekPreviewSize::Smallest,
            ],
        );
        settings.still_seek_preview_size_values.smallest =
            if settings.still_seek_preview_size_values.smallest
                == STILL_SEEK_PREVIEW_HEIGHT_MAX_POINTS
            {
                STILL_SEEK_PREVIEW_HEIGHT_MIN_POINTS
            } else {
                STILL_SEEK_PREVIEW_HEIGHT_MAX_POINTS
            };
        settings.still_seek_preview_size_values.small = if settings
            .still_seek_preview_size_values
            .small
            == STILL_SEEK_PREVIEW_HEIGHT_MAX_POINTS
        {
            STILL_SEEK_PREVIEW_HEIGHT_MIN_POINTS
        } else {
            STILL_SEEK_PREVIEW_HEIGHT_MAX_POINTS
        };
        settings.still_seek_preview_size_values.medium =
            if settings.still_seek_preview_size_values.medium
                == STILL_SEEK_PREVIEW_HEIGHT_MAX_POINTS
            {
                STILL_SEEK_PREVIEW_HEIGHT_MIN_POINTS
            } else {
                STILL_SEEK_PREVIEW_HEIGHT_MAX_POINTS
            };
        settings.still_seek_preview_size_values.large = if settings
            .still_seek_preview_size_values
            .large
            == STILL_SEEK_PREVIEW_HEIGHT_MAX_POINTS
        {
            STILL_SEEK_PREVIEW_HEIGHT_MIN_POINTS
        } else {
            STILL_SEEK_PREVIEW_HEIGHT_MAX_POINTS
        };
        settings.still_seek_preview_size_values.maximum =
            if settings.still_seek_preview_size_values.maximum
                == STILL_SEEK_PREVIEW_HEIGHT_MAX_POINTS
            {
                STILL_SEEK_PREVIEW_HEIGHT_MIN_POINTS
            } else {
                STILL_SEEK_PREVIEW_HEIGHT_MAX_POINTS
            };
        settings.still_seek_hover_preview_mode = different_enum(
            &settings.still_seek_hover_preview_mode,
            &[
                StillSeekHoverPreviewMode::Always,
                StillSeekHoverPreviewMode::HideWithThumbnailStrip,
                StillSeekHoverPreviewMode::Never,
            ],
        );
        settings.still_seek_bar_with_strip = different_enum(
            &settings.still_seek_bar_with_strip,
            &[StillSeekBarWithStrip::Show, StillSeekBarWithStrip::Hide],
        );
        settings.video_volume = if settings.video_volume == VIDEO_VOLUME_MAX {
            0.0
        } else {
            VIDEO_VOLUME_MAX
        };
        settings.video_seek_small_secs = if settings.video_seek_small_secs == VIDEO_SEEK_SECONDS_MAX
        {
            VIDEO_SEEK_SECONDS_MIN
        } else {
            VIDEO_SEEK_SECONDS_MAX
        };
        settings.video_seek_medium_secs =
            if settings.video_seek_medium_secs == VIDEO_SEEK_SECONDS_MAX {
                VIDEO_SEEK_SECONDS_MIN
            } else {
                VIDEO_SEEK_SECONDS_MAX
            };
        settings.video_seek_large_secs = if settings.video_seek_large_secs == VIDEO_SEEK_SECONDS_MAX
        {
            VIDEO_SEEK_SECONDS_MIN
        } else {
            VIDEO_SEEK_SECONDS_MAX
        };
        settings.video_seek_thumbnail_tolerance_secs = if settings
            .video_seek_thumbnail_tolerance_secs
            == VIDEO_SEEK_THUMBNAIL_TOLERANCE_MAX_SECS
        {
            VIDEO_SEEK_THUMBNAIL_TOLERANCE_MIN_SECS
        } else {
            VIDEO_SEEK_THUMBNAIL_TOLERANCE_MAX_SECS
        };
        settings.video_seek_strip_min_interval_secs = if settings.video_seek_strip_min_interval_secs
            == VIDEO_SEEK_STRIP_MIN_INTERVAL_MAX_SECS
        {
            VIDEO_SEEK_STRIP_MIN_INTERVAL_MIN_SECS
        } else {
            VIDEO_SEEK_STRIP_MIN_INTERVAL_MAX_SECS
        };
        settings.video_seek_strip_waveform_span_secs = if settings
            .video_seek_strip_waveform_span_secs
            == VIDEO_SEEK_STRIP_WAVEFORM_SPAN_MAX_SECS
        {
            VIDEO_SEEK_STRIP_WAVEFORM_SPAN_MIN_SECS
        } else {
            VIDEO_SEEK_STRIP_WAVEFORM_SPAN_MAX_SECS
        };
        settings.video_seek_strip_height = different_enum(
            &settings.video_seek_strip_height,
            &[
                crate::video::seek_strip_layout::SeekStripHeight::Maximum,
                crate::video::seek_strip_layout::SeekStripHeight::Large,
                crate::video::seek_strip_layout::SeekStripHeight::Medium,
                crate::video::seek_strip_layout::SeekStripHeight::Small,
                crate::video::seek_strip_layout::SeekStripHeight::Smallest,
            ],
        );
        settings.video_seek_preview_size = different_enum(
            &settings.video_seek_preview_size,
            &[
                VideoSeekPreviewSize::Maximum,
                VideoSeekPreviewSize::Large,
                VideoSeekPreviewSize::Medium,
                VideoSeekPreviewSize::Small,
                VideoSeekPreviewSize::Smallest,
            ],
        );
        settings.video_seek_preview_size_values.smallest =
            if settings.video_seek_preview_size_values.smallest
                == VIDEO_SEEK_PREVIEW_WIDTH_MAX_POINTS
            {
                VIDEO_SEEK_PREVIEW_WIDTH_MIN_POINTS
            } else {
                VIDEO_SEEK_PREVIEW_WIDTH_MAX_POINTS
            };
        settings.video_seek_preview_size_values.small =
            if settings.video_seek_preview_size_values.small == VIDEO_SEEK_PREVIEW_WIDTH_MAX_POINTS
            {
                VIDEO_SEEK_PREVIEW_WIDTH_MIN_POINTS
            } else {
                VIDEO_SEEK_PREVIEW_WIDTH_MAX_POINTS
            };
        settings.video_seek_preview_size_values.medium = if settings
            .video_seek_preview_size_values
            .medium
            == VIDEO_SEEK_PREVIEW_WIDTH_MAX_POINTS
        {
            VIDEO_SEEK_PREVIEW_WIDTH_MIN_POINTS
        } else {
            VIDEO_SEEK_PREVIEW_WIDTH_MAX_POINTS
        };
        settings.video_seek_preview_size_values.large =
            if settings.video_seek_preview_size_values.large == VIDEO_SEEK_PREVIEW_WIDTH_MAX_POINTS
            {
                VIDEO_SEEK_PREVIEW_WIDTH_MIN_POINTS
            } else {
                VIDEO_SEEK_PREVIEW_WIDTH_MAX_POINTS
            };
        settings.video_seek_preview_size_values.maximum =
            if settings.video_seek_preview_size_values.maximum
                == VIDEO_SEEK_PREVIEW_WIDTH_MAX_POINTS
            {
                VIDEO_SEEK_PREVIEW_WIDTH_MIN_POINTS
            } else {
                VIDEO_SEEK_PREVIEW_WIDTH_MAX_POINTS
            };
        settings.video_seek_strip_height_values.smallest =
            if settings.video_seek_strip_height_values.smallest
                == crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MAX_POINTS
            {
                crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MIN_POINTS
            } else {
                crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MAX_POINTS
            };
        settings.video_seek_strip_height_values.small =
            if settings.video_seek_strip_height_values.small
                == crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MAX_POINTS
            {
                crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MIN_POINTS
            } else {
                crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MAX_POINTS
            };
        settings.video_seek_strip_height_values.medium =
            if settings.video_seek_strip_height_values.medium
                == crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MAX_POINTS
            {
                crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MIN_POINTS
            } else {
                crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MAX_POINTS
            };
        settings.video_seek_strip_height_values.large =
            if settings.video_seek_strip_height_values.large
                == crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MAX_POINTS
            {
                crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MIN_POINTS
            } else {
                crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MAX_POINTS
            };
        settings.video_seek_strip_height_values.maximum =
            if settings.video_seek_strip_height_values.maximum
                == crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MAX_POINTS
            {
                crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MIN_POINTS
            } else {
                crate::video::seek_strip_layout::VIDEO_SEEK_STRIP_HEIGHT_MAX_POINTS
            };
        settings.video_seek_strip_cycle.thumbnails_window =
            !settings.video_seek_strip_cycle.thumbnails_window;
        settings.video_seek_strip_cycle.thumbnails_whole =
            !settings.video_seek_strip_cycle.thumbnails_whole;
        settings.video_seek_strip_cycle.waveform_window =
            !settings.video_seek_strip_cycle.waveform_window;
        settings.video_seek_strip_cycle.waveform_whole =
            !settings.video_seek_strip_cycle.waveform_whole;
        // UI requires at least one candidate; flipping the all-enabled default
        // must produce a valid, different value rather than an empty set.
        settings.video_seek_strip_cycle = settings.video_seek_strip_cycle.normalized();
        settings.video_top_bar_locked = !settings.video_top_bar_locked;
        settings.video_seek_hover_preview_mode = different_enum(
            &settings.video_seek_hover_preview_mode,
            &[
                VideoSeekHoverPreviewMode::Always,
                VideoSeekHoverPreviewMode::HideWithThumbnailStrip,
                VideoSeekHoverPreviewMode::Never,
            ],
        );
        settings.video_seek_bar_with_strip = different_enum(
            &settings.video_seek_bar_with_strip,
            &[VideoSeekBarWithStrip::Show, VideoSeekBarWithStrip::Hide],
        );
        settings.video_loop_mode = different_enum(
            &settings.video_loop_mode,
            &[
                VideoLoopMode::Off,
                VideoLoopMode::Full,
                VideoLoopMode::Chapter,
                VideoLoopMode::Bookmark,
            ],
        );
        settings.video_start_muted = !settings.video_start_muted;
        settings.video_thumb_use_sidecar_image = !settings.video_thumb_use_sidecar_image;
        settings.video_grid_open_starts_from_beginning =
            !settings.video_grid_open_starts_from_beginning;
        settings.video_nav_resume = different_enum(
            &settings.video_nav_resume,
            &[ResumeMode::Resume, ResumeMode::FromStart],
        );
        settings.book_open_resume = different_enum(
            &settings.book_open_resume,
            &[ResumeMode::Resume, ResumeMode::FromStart],
        );
        settings.book_nav_resume = different_enum(
            &settings.book_nav_resume,
            &[ResumeMode::Resume, ResumeMode::FromStart],
        );
        settings.music_open_resume = different_enum(
            &settings.music_open_resume,
            &[ResumeMode::Resume, ResumeMode::FromStart],
        );
        settings.music_nav_resume = different_enum(
            &settings.music_nav_resume,
            &[ResumeMode::Resume, ResumeMode::FromStart],
        );
        settings.set_still_bottom_lock(BottomBarLock::BarAndStrip);
        settings.set_still_seek_strip_visible(true);
        settings
    }

    #[test]
    fn all_settings_fields_are_classified() {
        let entries = classifications();
        // video_watched_to_end is already an excluded field in the base policy.
        assert_eq!(entries.len(), 441);
        assert_eq!(
            entries
                .iter()
                .filter(|(_, reason)| reason.is_none())
                .count(),
            131
        );
        let unique: HashSet<_> = entries.iter().map(|(key, _)| key).collect();
        assert_eq!(unique.len(), entries.len());
        assert!(
            entries
                .iter()
                .all(|(_, reason)| reason.is_none_or(|reason| !reason.is_empty()))
        );
        let wire = wire_keys();
        assert_eq!(wire.len(), 129);
        assert_eq!(wire.iter().collect::<HashSet<_>>().len(), wire.len());
        let exported = export_preferences(&Settings::default()).unwrap();
        assert!(exported.issues.is_empty(), "{:?}", exported.issues);
        assert_eq!(exported.item_count, wire.len());
        let value: Value = serde_json::from_str(&exported.json).unwrap();
        assert_eq!(
            value["preferences"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<HashSet<_>>(),
            wire.into_iter().collect()
        );
    }

    #[test]
    fn all_allowed_values_roundtrip_and_all_excluded_fields_preserve() {
        let source = nondefault_source();
        let exported = export_preferences(&source).unwrap();
        assert!(exported.issues.is_empty(), "{:?}", exported.issues);
        let mut destination = private_destination();
        let before = destination.clone();
        let parsed = parse_preferences(&exported.json).unwrap();
        let report = parsed.apply_to(&mut destination);
        assert_eq!(report.accepted_count, wire_keys().len());
        assert!(report.issues.is_empty());
        assert_excluded_unchanged(&before, &destination);
        assert_eq!(
            export_preferences(&destination).unwrap().json,
            exported.json
        );
        assert_eq!(
            destination.archive_convert_without_dialog,
            source.archive_file_handling == ArchiveFileHandling::Convert
        );
        assert_eq!(
            destination.video_loop,
            source.video_loop_mode != VideoLoopMode::Off
        );
        assert_eq!(parsed.apply_to(&mut destination).changed_fields.len(), 0);
        assert!(
            report
                .changed_fields
                .iter()
                .all(|label| label.chars().any(|c| !c.is_ascii()))
        );
    }

    fn private_destination() -> Settings {
        let sentinel = r"C:\Users\private-alice\SECRET_PIN_184729";
        let mut settings = Settings::default();
        settings.last_folder = Some(sentinel.into());
        settings.startup_folder_path = Some(sentinel.into());
        settings.capture_output_dir = Some(sentinel.into());
        settings.last_cursor_name = Some("private-book-and-tag-name".into());
        settings.recent_folders = vec![sentinel.into(), r"\\private-server\personal".into()];
        settings.quick_folder_slots = [Some(sentinel.into()), None];
        settings.quick_folder_drive_current_dirs[0].insert("Z:".into(), sentinel.into());
        settings.active_book_name = "private-collection".into();
        settings.exif_hidden_tags = vec!["https://secret.example/personal".into()];
        settings.vst3_plugin_path = Some(sentinel.into());
        settings.vst3_plugin_state = Some("SECRET_PIN_184729".into());
        settings.vst3_plugins = vec![Vst3PluginEntry {
            path: sentinel.into(),
            bypass: false,
            state: Some("SECRET_PIN_184729".into()),
            user_hidden: true,
            gui_pos: None,
            gui_size: None,
        }];
        settings
            .video_resume_positions
            .insert(sentinel.into(), 88.5);
        settings.video_watched_to_end.insert(sentinel.into());
        settings.reading_history_limit = 17;
        settings.remote_service_enabled = true;
        settings.remote_video_streaming_enabled = true;
        settings.details_selection_bar_mode = DetailsSelectionBarMode::Dedicated;
        settings.stack_script_enabled = true;
        settings.ui_font.selection = UiFontSelection::Face {
            display_name: "private-font".into(),
            path: sentinel.into(),
            face_index: 3,
            post_script_name: "private-font".into(),
        };
        settings.file_organize_destinations = vec![FileOrganizeDestination {
            name: "private-target".into(),
            path: sentinel.into(),
        }];
        settings.favorites = vec![FavoriteEntry {
            id: uuid::Uuid::new_v4(),
            name: "private-favorite".into(),
            path: sentinel.into(),
            auto_index_structure: true,
            auto_index_metadata: true,
            auto_index_thumbs: true,
            auto_index_similar: true,
        }];
        settings.tags = vec![TagDef::new("private-tag".into())];
        settings.smart_folders = vec![SmartFolderDefinition::new("private-smart-folder")];
        settings.pinned_books = vec!["private-book".into()];
        settings.creative_luts = vec![crate::creative_lut::CreativeLutEntry {
            id: uuid::Uuid::new_v4(),
            name: "private-lut".into(),
            path: sentinel.into(),
            builtin: None,
        }];
        settings.favorite_view_overlay = Some(FavoriteViewOverlay {
            favorite_id: uuid::Uuid::new_v4(),
            common: FavoriteViewState::from_settings(&settings),
        });
        settings
    }

    #[test]
    fn excluded_private_values_never_reach_export_or_diagnostics() {
        let mut settings = private_destination();
        settings
            .image_ext_priority
            .push(r"C:\Users\private-alice\SECRET_PIN_184729".into());
        let exported = export_preferences(&settings).unwrap();
        let diagnostic = format!("{:?}", exported.issues);
        for needle in [
            "private-alice",
            "SECRET_PIN",
            "private-book",
            "private-server",
            "private-collection",
            "private-tag",
            "private-favorite",
            "private-lut",
            "private-smart-folder",
            "secret.example",
        ] {
            assert!(!exported.json.contains(needle));
            assert!(!diagnostic.contains(needle));
        }
        assert_eq!(exported.issues.len(), 1);
        let value: Value = serde_json::from_str(&exported.json).unwrap();
        assert!(value["preferences"].get("image_ext_priority").is_none());
        let mut destination = settings.clone();
        parse_preferences(&exported.json)
            .unwrap()
            .apply_to(&mut destination);
        assert_excluded_unchanged(&settings, &destination);
    }

    #[test]
    fn snapshot_owns_only_captured_values_and_keeps_draft_unchanged() {
        let mut settings = private_destination();
        settings.ui_theme = UiTheme::Dark;
        let snapshot = capture_export(&settings);
        settings.ui_theme = UiTheme::Light;
        let result = snapshot.export().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&result.json).unwrap()["preferences"]["ui_theme"],
            "Dark"
        );
        assert_eq!(settings.ui_theme, UiTheme::Light);
    }

    #[test]
    fn master_preferences_merge_transfers_raw_policy_and_preserves_local_state() {
        let mut source = Settings::default();
        source.raw_brightness = crate::raw::RawBrightness::None;
        source.raw_develop_parallelism = 10;
        source.active_quick_folder_slot = None;
        source.show_facet_sort = true;
        source.toolbar_folder_section_migrated = false;
        source.effetune_pre_limiter_enabled = false;
        source.effetune_keep_visible_when_minimized = true;
        source.window_pos = Some([100.0, 200.0]);
        source.toolbar_section_order.reverse();
        let exported = export_preferences(&source).unwrap();
        let mut document: Value = serde_json::from_str(&exported.json).unwrap();
        assert_eq!(document["preferences"]["raw_brightness"], "None");
        let excluded = [
            ("raw_develop_parallelism", serde_json::json!(10)),
            ("active_quick_folder_slot", Value::Null),
            ("show_facet_sort", serde_json::json!(true)),
            ("toolbar_folder_section_migrated", serde_json::json!(false)),
            ("effetune_pre_limiter_enabled", serde_json::json!(false)),
            (
                "effetune_keep_visible_when_minimized",
                serde_json::json!(true),
            ),
            ("window_pos", serde_json::json!([100.0, 200.0])),
            (
                "toolbar_section_order",
                serde_json::to_value(&source.toolbar_section_order).unwrap(),
            ),
            ("toolbar_section_new_row", serde_json::json!([])),
            ("show_toolbar_folder", serde_json::json!(false)),
            ("show_address_bar_history_nav", serde_json::json!(false)),
        ];
        for (key, value) in excluded {
            assert!(document["preferences"].get(key).is_none(), "{key}");
            document["preferences"][key] = value;
        }
        let mut destination = Settings::default();
        let before = destination.clone();
        let parsed = parse_preferences(&document.to_string()).unwrap();
        let report = parsed.apply_to(&mut destination);
        assert_eq!(report.unknown_count, 11);
        assert_eq!(destination.raw_brightness, crate::raw::RawBrightness::None);
        assert_excluded_unchanged(&before, &destination);

        document["preferences"]["raw_brightness"] = serde_json::json!("FutureBrightness");
        let parsed = parse_preferences(&document.to_string()).unwrap();
        let report = parsed.apply_to(&mut destination);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(destination.raw_brightness, crate::raw::RawBrightness::None);
        assert_excluded_unchanged(&before, &destination);
    }

    #[test]
    fn automatic_network_enablement_is_explicitly_excluded_from_export_and_import() {
        // Independent of classifications()/wire_keys() and the policy-generated assertions.
        let mut source = Settings::default();
        source.remote_service_enabled = true;
        source.remote_video_streaming_enabled = true;
        source.update_check_enabled = true;
        let exported = export_preferences(&source).unwrap();
        let document: Value = serde_json::from_str(&exported.json).unwrap();
        let preferences = document["preferences"].as_object().unwrap();
        assert!(!preferences.contains_key("remote_service_enabled"));
        assert!(!preferences.contains_key("remote_video_streaming_enabled"));
        assert!(!preferences.contains_key("update_check_enabled"));

        let mut destination = Settings::default();
        destination.remote_service_enabled = false;
        destination.remote_video_streaming_enabled = false;
        destination.update_check_enabled = false;
        let parsed = parse_preferences(
            r#"{"format":"mimageviewer.preferences","format_version":1,"preferences":{"remote_service_enabled":true,"remote_video_streaming_enabled":true,"update_check_enabled":true}}"#,
        )
        .unwrap();
        parsed.apply_to(&mut destination);
        assert!(!destination.remote_service_enabled);
        assert!(!destination.remote_video_streaming_enabled);
        assert!(!destination.update_check_enabled);
    }

    #[test]
    fn initial_v1_missing_and_unknown_keys_keep_destination() {
        let mut destination = private_destination();
        destination.slideshow_interval_secs = 11.0;
        let parsed = parse_preferences(INITIAL_V1).unwrap();
        let report = parsed.apply_to(&mut destination);
        assert_eq!(report.accepted_count, 4);
        assert_eq!(destination.ui_theme, UiTheme::Dark);
        assert!(destination.show_hidden_files);
        assert_eq!(destination.video_seek_small_secs, 3);
        assert_eq!(destination.slideshow_interval_secs, 11.0);
        let parsed = parse_preferences(&document(json!({"future_key":"SECRET_PIN", "details_selection_bar_mode":"SameAsDetails", "stack_script_enabled":false, "fullscreen_seek_bar_locked":false, "still_seek_strip_locked":true, "still_seek_strip_visible":false}))).unwrap();
        assert_eq!(parsed.unknown_count, 6);
        let before = destination.clone();
        assert_eq!(parsed.apply_to(&mut destination).accepted_count, 0);
        assert_excluded_unchanged(&before, &destination);
        assert_eq!(destination.still_bottom_lock(), before.still_bottom_lock());
        assert_eq!(
            destination.still_seek_strip_visible,
            before.still_seek_strip_visible
        );
    }

    #[test]
    fn bad_items_skip_without_echo_or_default_substitution() {
        let cases = [
            ("ui_theme", json!("Standard")),
            ("ui_theme", json!("SECRET_PIN")),
            ("text_contrast", json!("unknown")),
            ("fullscreen_side_panel_mode", json!("Unknown")),
            ("panorama_projection", json!("unknown")),
            ("fullscreen_fit_mode", json!("MarginFit")),
            ("default_spread_mode", json!("Vertical")),
            ("folder_thumb_sort", json!("SizeAsc")),
            ("show_hidden_files", json!(1)),
            ("show_hidden_files", Value::Null),
            ("folder_thumb_depth", json!(-1)),
            ("folder_thumb_depth", json!(1.0)),
            ("video_seek_small_secs", json!(u64::MAX)),
            ("slideshow_interval_secs", json!("2.0")),
            ("fullscreen_image_margin_color", json!([0, 0, 0, 0])),
            ("fullscreen_image_margin_color", json!([0, 0, 256])),
            ("archive_file_handling", json!("Legacy")),
            (
                "image_ext_priority",
                json!(["jpg", "https://secret.example/SECRET_PIN"]),
            ),
        ];
        for (key, invalid) in cases {
            let mut items = Map::new();
            items.insert(key.into(), invalid);
            items.insert("restore_last_cursor".into(), json!(false));
            let mut settings = private_destination();
            let before = export_preferences(&settings).unwrap().json;
            let parsed = parse_preferences(&document(Value::Object(items))).unwrap();
            assert_eq!(parsed.issues.len(), 1, "{key}");
            let report = parsed.apply_to(&mut settings);
            assert_eq!(report.accepted_count, 1, "{key}");
            assert!(!settings.restore_last_cursor);
            settings.restore_last_cursor = true;
            assert_eq!(before, export_preferences(&settings).unwrap().json, "{key}");
            assert!(!format!("{:?}", parsed.issues).contains("SECRET_PIN"));
        }
    }

    #[test]
    fn strict_composites_do_not_fill_missing_or_repair_invalid_children() {
        let valid =
            serde_json::from_str::<Value>(&export_preferences(&Settings::default()).unwrap().json)
                .unwrap()["preferences"]
                .clone();
        for key in [
            "still_seek_strip_height_values",
            "still_seek_preview_size_values",
            "video_seek_preview_size_values",
            "video_seek_strip_height_values",
            "video_seek_strip_cycle",
            "still_bottom_chrome",
        ] {
            let mut incomplete = valid[key].clone();
            let first = incomplete
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .clone();
            incomplete.as_object_mut().unwrap().remove(&first);
            let parsed = parse_preferences(&document(json!({key: incomplete}))).unwrap();
            assert_eq!(parsed.accepted.len(), 0, "{key}");
            assert_eq!(parsed.issues.len(), 1, "{key}");
            let mut extended = valid[key].clone();
            extended
                .as_object_mut()
                .unwrap()
                .insert("future_child".into(), json!("SECRET_PIN"));
            assert_eq!(
                parse_preferences(&document(json!({key: extended})))
                    .unwrap()
                    .accepted
                    .len(),
                1,
                "{key}"
            );
        }
        for grid in [
            json!([]),
            json!([["folder"], ["archive"], ["image"]]),
            json!([
                ["folder", "folder", "archive"],
                ["image", "video_audio"],
                [],
                []
            ]),
            json!([["folder", "archive"], ["image", "unknown"], [], []]),
        ] {
            assert_eq!(
                parse_preferences(&document(json!({"grid_display_order":grid})))
                    .unwrap()
                    .issues
                    .len(),
                1
            );
        }
        assert_eq!(parse_preferences(&document(json!({"video_seek_strip_cycle":{"thumbnails_window":false,"thumbnails_whole":false,"waveform_window":false,"waveform_whole":false}}))).unwrap().issues.len(), 1);
        for key in [
            "still_seek_strip_height_values",
            "still_seek_preview_size_values",
            "video_seek_preview_size_values",
            "video_seek_strip_height_values",
        ] {
            let mut value = valid[key].clone();
            value["smallest"] = json!(0);
            assert_eq!(
                parse_preferences(&document(json!({key:value})))
                    .unwrap()
                    .issues
                    .len(),
                1
            );
        }
    }

    #[test]
    fn null_is_rejected_for_every_logical_item() {
        for key in wire_keys() {
            let parsed = parse_preferences(&document(json!({key: null}))).unwrap();
            assert_eq!(parsed.accepted.len(), 0, "{key}");
            assert_eq!(parsed.issues.len(), 1, "{key}");
        }
    }

    #[test]
    fn numeric_limits_accept_endpoints_and_skip_values_outside() {
        let ranges: Vec<(&str, f64, f64)> = vec![
            (
                "slideshow_interval_secs",
                (0.5).to_string().parse().unwrap(),
                (30.0).to_string().parse().unwrap(),
            ),
            (
                "slideshow_continuous_wait_secs",
                (0.1).to_string().parse().unwrap(),
                (30.0).to_string().parse().unwrap(),
            ),
            (
                "slideshow_continuous_scroll_secs",
                (0.0).to_string().parse().unwrap(),
                (5.0).to_string().parse().unwrap(),
            ),
            (
                "slideshow_continuous_scroll_percent",
                (1).to_string().parse().unwrap(),
                (100).to_string().parse().unwrap(),
            ),
            (
                "folder_thumb_depth",
                (0).to_string().parse().unwrap(),
                (10).to_string().parse().unwrap(),
            ),
            (
                "folder_skip_limit",
                (1).to_string().parse().unwrap(),
                (30).to_string().parse().unwrap(),
            ),
            (
                "spread_page_gap_px",
                (0).to_string().parse().unwrap(),
                (200).to_string().parse().unwrap(),
            ),
            (
                "continuous_reading_gap_px",
                (0).to_string().parse().unwrap(),
                (200).to_string().parse().unwrap(),
            ),
            (
                "fullscreen_fixed_bar_gap_px",
                (0).to_string().parse().unwrap(),
                (FULLSCREEN_FIXED_BAR_GAP_MAX_PX)
                    .to_string()
                    .parse()
                    .unwrap(),
            ),
            (
                "fullscreen_cursor_hide_delay_secs",
                (FULLSCREEN_CURSOR_HIDE_DELAY_MIN_SECS)
                    .to_string()
                    .parse()
                    .unwrap(),
                (FULLSCREEN_CURSOR_HIDE_DELAY_MAX_SECS)
                    .to_string()
                    .parse()
                    .unwrap(),
            ),
            (
                "fullscreen_jump_percent",
                (FULLSCREEN_JUMP_PERCENT_MIN).to_string().parse().unwrap(),
                (FULLSCREEN_JUMP_PERCENT_MAX).to_string().parse().unwrap(),
            ),
            (
                "fullscreen_fixed_jump_count",
                (FULLSCREEN_FIXED_JUMP_MIN).to_string().parse().unwrap(),
                (FULLSCREEN_FIXED_JUMP_MAX).to_string().parse().unwrap(),
            ),
            (
                "continuous_reading_wheel_scroll_percent",
                (1).to_string().parse().unwrap(),
                (100).to_string().parse().unwrap(),
            ),
            (
                "continuous_reading_key_scroll_percent",
                (1).to_string().parse().unwrap(),
                (100).to_string().parse().unwrap(),
            ),
            (
                "continuous_reading_gamepad_scroll_percent_per_sec",
                (10).to_string().parse().unwrap(),
                (300).to_string().parse().unwrap(),
            ),
            (
                "video_volume",
                (0.0).to_string().parse().unwrap(),
                (VIDEO_VOLUME_MAX).to_string().parse().unwrap(),
            ),
            (
                "video_seek_small_secs",
                (VIDEO_SEEK_SECONDS_MIN).to_string().parse().unwrap(),
                (VIDEO_SEEK_SECONDS_MAX).to_string().parse().unwrap(),
            ),
            (
                "video_seek_medium_secs",
                (VIDEO_SEEK_SECONDS_MIN).to_string().parse().unwrap(),
                (VIDEO_SEEK_SECONDS_MAX).to_string().parse().unwrap(),
            ),
            (
                "video_seek_large_secs",
                (VIDEO_SEEK_SECONDS_MIN).to_string().parse().unwrap(),
                (VIDEO_SEEK_SECONDS_MAX).to_string().parse().unwrap(),
            ),
            (
                "video_seek_thumbnail_tolerance_secs",
                (VIDEO_SEEK_THUMBNAIL_TOLERANCE_MIN_SECS)
                    .to_string()
                    .parse()
                    .unwrap(),
                (VIDEO_SEEK_THUMBNAIL_TOLERANCE_MAX_SECS)
                    .to_string()
                    .parse()
                    .unwrap(),
            ),
            (
                "video_seek_strip_min_interval_secs",
                (VIDEO_SEEK_STRIP_MIN_INTERVAL_MIN_SECS)
                    .to_string()
                    .parse()
                    .unwrap(),
                (VIDEO_SEEK_STRIP_MIN_INTERVAL_MAX_SECS)
                    .to_string()
                    .parse()
                    .unwrap(),
            ),
            (
                "video_seek_strip_waveform_span_secs",
                (VIDEO_SEEK_STRIP_WAVEFORM_SPAN_MIN_SECS)
                    .to_string()
                    .parse()
                    .unwrap(),
                (VIDEO_SEEK_STRIP_WAVEFORM_SPAN_MAX_SECS)
                    .to_string()
                    .parse()
                    .unwrap(),
            ),
        ];
        for (key, min, max) in ranges {
            let integer = matches!(
                key,
                "slideshow_continuous_scroll_percent"
                    | "folder_thumb_depth"
                    | "folder_skip_limit"
                    | "spread_page_gap_px"
                    | "continuous_reading_gap_px"
                    | "fullscreen_fixed_bar_gap_px"
                    | "fullscreen_jump_percent"
                    | "fullscreen_fixed_jump_count"
                    | "continuous_reading_wheel_scroll_percent"
                    | "continuous_reading_key_scroll_percent"
                    | "continuous_reading_gamepad_scroll_percent_per_sec"
                    | "video_seek_small_secs"
                    | "video_seek_medium_secs"
                    | "video_seek_large_secs"
            );
            for endpoint in [min, max] {
                let value = if integer {
                    json!(endpoint as u64)
                } else {
                    json!(endpoint)
                };
                let parsed = parse_preferences(&document(json!({key:value}))).unwrap();
                assert_eq!(parsed.accepted.len(), 1, "endpoint {key}: {endpoint}");
            }
            let step = if integer { 1.0 } else { 0.00001 };
            for outside in [min - step, max + step] {
                let value = if integer {
                    json!(outside as i64)
                } else {
                    json!(outside)
                };
                let parsed = parse_preferences(&document(json!({key:value}))).unwrap();
                assert_eq!(parsed.accepted.len(), 0, "outside {key}: {outside}");
                assert_eq!(parsed.issues.len(), 1, "{key}");
            }
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut settings = Settings::default();
            settings.video_volume = value;
            assert_eq!(export_preferences(&settings).unwrap().issues.len(), 1);
        }
    }

    #[test]
    fn still_bottom_chrome_all_reachable_states_and_invalid_preservation() {
        for lock in ["None", "BarOnly", "BarAndStrip"] {
            for visible in [false, true] {
                let mut settings = private_destination();
                settings.set_still_bottom_lock(BottomBarLock::BarAndStrip);
                settings.set_still_seek_strip_visible(true);
                let parsed = parse_preferences(&document(
                    json!({"still_bottom_chrome":{"lock":lock,"visible":visible}}),
                ))
                .unwrap();
                let report = parsed.apply_to(&mut settings);
                if lock == "BarAndStrip" && !visible {
                    assert_eq!(report.accepted_count, 0);
                    assert_eq!(settings.still_bottom_lock(), BottomBarLock::BarAndStrip);
                    assert!(settings.still_seek_strip_visible);
                } else {
                    assert_eq!(report.accepted_count, 1);
                    assert_eq!(settings.still_seek_strip_visible, visible);
                    assert_eq!(format!("{:?}", settings.still_bottom_lock()), lock);
                    let exported = export_preferences(&settings).unwrap();
                    assert!(exported.issues.is_empty());
                }
            }
        }
        let mut invalid = Settings::default();
        invalid.fullscreen_seek_bar_locked = false;
        invalid.still_seek_strip_locked = true;
        assert_eq!(export_preferences(&invalid).unwrap().issues.len(), 1);
    }

    #[test]
    fn archive_resolution_and_loop_mirrors_are_explicit() {
        for (legacy, expected) in [(false, "Ask"), (true, "Convert")] {
            let mut settings = Settings::default();
            settings.archive_file_handling = ArchiveFileHandling::Legacy;
            settings.archive_convert_without_dialog = legacy;
            let result = export_preferences(&settings).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&result.json).unwrap()["preferences"]["archive_file_handling"],
                expected
            );
        }
        let mut settings = Settings::default();
        parse_preferences(&document(
            json!({"archive_file_handling":"Convert","video_loop_mode":"Chapter"}),
        ))
        .unwrap()
        .apply_to(&mut settings);
        assert!(settings.archive_convert_without_dialog);
        assert!(settings.video_loop);
        parse_preferences(&document(
            json!({"archive_file_handling":"Ignore","video_loop_mode":"Off"}),
        ))
        .unwrap()
        .apply_to(&mut settings);
        assert!(!settings.archive_convert_without_dialog);
        assert!(!settings.video_loop);
    }

    #[test]
    fn invalid_whole_documents_versions_duplicate_keys_and_depth_reject() {
        for input in [
            "",
            "{",
            "[]",
            "null",
            r#"{"format":"other","format_version":1,"preferences":{}}"#,
            r#"{"format":"mimageviewer.preferences","preferences":{}}"#,
            r#"{"format":"mimageviewer.preferences","format_version":1,"preferences":[],"format_version":1}"#,
            r#"{"format":"mimageviewer.preferences","format_version":1,"preferences":{"unknown":{"a":1,"a":2}}}"#,
        ] {
            assert!(parse_preferences(input).is_err(), "{input}");
        }
        for version in [
            json!(0),
            json!(2),
            json!(-1),
            json!(1.0),
            json!("1"),
            Value::Null,
            json!(u64::MAX),
        ] {
            assert!(
                parse_preferences(
                    &json!({"format":FORMAT,"format_version":version,"preferences":{}}).to_string()
                )
                .is_err()
            );
        }
        let deep = format!("{}0{}", "[".repeat(160), "]".repeat(160));
        assert!(parse_preferences(&deep).is_err());
        assert!(parse_preferences(&" ".repeat(MAX_PREFERENCES_BYTES + 1)).is_err());
        assert!(parse_preferences(&format!("\u{feff}{INITIAL_V1}")).is_ok());
    }

    #[test]
    fn atomic_files_roundtrip_replace_and_fail_without_damage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let first = export_preferences(&Settings::default()).unwrap();
        write_preferences(&path, &first.json).unwrap();
        let second = export_preferences(&nondefault_source()).unwrap();
        write_preferences(&path, &second.json).unwrap();
        let mut settings = Settings::default();
        read_preferences(&path).unwrap().apply_to(&mut settings);
        assert_eq!(export_preferences(&settings).unwrap().json, second.json);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let locked = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&path)
                .unwrap();
            assert!(write_preferences(&path, &first.json).is_err());
            drop(locked);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), second.json);
        }
        let folder = dir.path().join("existing-directory");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("sentinel"), "unchanged").unwrap();
        assert!(write_preferences(&folder, &first.json).is_err());
        assert_eq!(
            std::fs::read_to_string(folder.join("sentinel")).unwrap(),
            "unchanged"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        assert!(
            write_preferences(
                &dir.path().join("missing-parent/preferences.json"),
                &first.json
            )
            .is_err()
        );
    }

    #[test]
    fn bounded_file_read_rejects_size_and_invalid_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(read_preferences(&path).is_err());
        std::fs::write(&path, vec![b' '; MAX_PREFERENCES_BYTES + 1]).unwrap();
        assert!(read_preferences(&path).is_err());
        std::fs::write(&path, format!("\u{feff}{INITIAL_V1}")).unwrap();
        assert!(read_preferences(&path).is_ok());
        assert!(read_preferences(&dir.path().join("absent.json")).is_err());
    }
}
