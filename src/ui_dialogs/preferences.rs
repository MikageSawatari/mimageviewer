//! 統合環境設定ダイアログ。
//!
//! 左側にツリー、右側に設定パネルを配置した環境設定ウィンドウ。
//! OK / キャンセルで一時コピーを確定 or 破棄する。

use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crate::app::App;
use crate::ring_shortcut::{
    MouseButtonSlot, MouseGestureDirection, RightDragContext, RingActionId, RingDirection,
    RingShortcutContext,
};
use crate::settings::{Parallelism, Settings};

mod pages;
mod search_index;
use self::pages::*;
use self::search_index::{PrefSearchEntry, search_preferences};

#[doc(hidden)]
pub fn draw_video_bar_visibility_snapshot_fixture(ui: &mut egui::Ui) {
    let mut settings = Settings {
        video_top_bar_locked: true,
        video_seek_bar_locked: false,
        video_seek_strip_locked: false,
        ..Settings::default()
    };
    pages::draw_video_bar_visibility_settings(ui, &mut settings);
}

#[doc(hidden)]
pub fn draw_video_thumbnail_indicator_settings_snapshot_fixture(ui: &mut egui::Ui) {
    let mut settings = Settings {
        video_thumbnail_indicator: crate::settings::VideoThumbnailIndicator::BottomLeftBadge,
        ..Settings::default()
    };
    pages::draw_video_thumbnail_indicator_settings(ui, &mut settings);
}

fn pref_panel_scroll_style() -> egui::style::ScrollStyle {
    let mut scroll = egui::style::ScrollStyle::solid();
    scroll.bar_width = 10.0;
    scroll.bar_inner_margin = 8.0;
    scroll.bar_outer_margin = 2.0;
    scroll.foreground_color = true;
    scroll
}

fn settings_equal_for_close_prompt(a: &Settings, b: &Settings) -> bool {
    match (serde_json::to_value(a), serde_json::to_value(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn draw_discard_changes_confirm(
    ctx: &egui::Context,
    id: &'static str,
    title: &str,
    message: &str,
) -> Option<bool> {
    let mut discard = false;
    let mut keep_editing = false;
    let response = egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        ui.set_min_width(420.0);
        ui.heading(title);
        ui.add_space(8.0);
        ui.label(message);
        ui.add_space(4.0);
        ui.label(egui::RichText::new("保存していない変更は元に戻せません。").weak());
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("破棄して閉じる").clicked() {
                discard = true;
            }
            if ui.button("編集に戻る").clicked() {
                keep_editing = true;
            }
        });
    });

    if discard {
        Some(true)
    } else if keep_editing || response.should_close() {
        Some(false)
    } else {
        None
    }
}

#[cfg(windows)]
pub enum Vst3ScanMessage {
    Progress {
        done: usize,
        total: usize,
        path: String,
    },
    Finished(Result<Vec<crate::video::dsp::DiscoveredPlugin>, String>),
}

// ── ページ列挙 ──────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PreferencesPage {
    General,
    Font,
    Startup,
    ExternalTools,
    ExplorerIntegration,
    Thumbnail,
    Slideshow,
    Capture,
    /// 静止画・動画で共用する Creative 3D LUT (.cube) の登録
    CreativeLut,
    MenuLayout,
    Parallelism,
    Prefetch,
    GpuMemory,
    /// AI 推論バックエンド (DirectML / TensorRT) の選択と TRT pack 管理
    AiBackend,
    Cache,
    Folder,
    /// v1.7.0: 製本（本棚の保存先ルート）
    Book,
    DuplicateFiles,
    ExifDisplay,
    SpreadMode,
    /// 履歴と復元 (閲覧履歴、読書/再生位置の復元)
    PlaybackResume,
    SusiePlugins,
    /// v0.8.0: 検索インデックスの速度プロファイル
    IndexerSpeed,
    /// v0.9: タスクトレイ常駐 / 常駐中 pause 設定
    TrayResidency,
    /// v0.8.1: レーティング XMP 書き込み設定
    Rating,
    /// 起動時 / 定期的なバージョン更新確認
    UpdateCheck,
    /// 動画再生 (HW デコード等)
    Video,
    /// VST3 プラグイン設定 (= 有効化 + チェーン編集)
    Vst3,
    /// 編集用追加パック (オノマトペ向けフォント + 被写体分離モデル) の管理
    EditingAddon,
    /// 開発者 / 診断 (ログ zip 書き出し・性能ログ)
    Developer,
}

impl PreferencesPage {
    #[cfg(test)]
    const ALL: &'static [Self] = &[
        Self::General,
        Self::Font,
        Self::Startup,
        Self::ExternalTools,
        Self::ExplorerIntegration,
        Self::Thumbnail,
        Self::Slideshow,
        Self::Capture,
        Self::CreativeLut,
        Self::MenuLayout,
        Self::Parallelism,
        Self::Prefetch,
        Self::GpuMemory,
        Self::AiBackend,
        Self::Cache,
        Self::Folder,
        Self::Book,
        Self::DuplicateFiles,
        Self::ExifDisplay,
        Self::SpreadMode,
        Self::PlaybackResume,
        Self::SusiePlugins,
        Self::IndexerSpeed,
        Self::TrayResidency,
        Self::Rating,
        Self::UpdateCheck,
        Self::Video,
        Self::Vst3,
        Self::EditingAddon,
        Self::Developer,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::General => "全体設定",
            Self::Font => "フォント",
            Self::Startup => "起動時の動作",
            Self::ExternalTools => "外部ツール",
            Self::ExplorerIntegration => "エクスプローラ連携",
            Self::Thumbnail => "サムネイル",
            Self::Slideshow => "スライドショー",
            Self::Capture => "キャプチャ保存",
            Self::CreativeLut => "LUT",
            Self::MenuLayout => "メニュー構成",
            Self::Parallelism => "並列読み込み",
            Self::Prefetch => "先読み",
            Self::GpuMemory => "GPUメモリ",
            Self::AiBackend => "AI バックエンド",
            Self::Cache => "キャッシュ",
            Self::Folder => "フォルダ・ファイル",
            Self::Book => "製本",
            Self::DuplicateFiles => "同名ファイル",
            Self::ExifDisplay => "EXIF表示",
            Self::SpreadMode => "閲覧表示",
            Self::PlaybackResume => "履歴と復元",
            Self::SusiePlugins => "Susie プラグイン",
            Self::IndexerSpeed => "検索インデックス",
            Self::TrayResidency => "タスクトレイ常駐",
            Self::Rating => "レーティング",
            Self::UpdateCheck => "更新確認",
            Self::Video => "動画",
            Self::Vst3 => "VST3 プラグイン",
            Self::EditingAddon => "編集用追加ファイル",
            Self::Developer => "開発者",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OperationCustomizeTab {
    Settings,
    Commands,
    RingShortcut,
    Keyboard,
    MouseGesture,
    Gamepad,
}

impl OperationCustomizeTab {
    fn label(self) -> &'static str {
        match self {
            Self::Settings => "設定",
            Self::Commands => "コマンド一覧",
            Self::RingShortcut => "リングショートカット",
            Self::Keyboard => "キーボード",
            Self::MouseGesture => "マウスジェスチャ",
            Self::Gamepad => "ゲームパッド",
        }
    }

    fn all() -> &'static [Self] {
        &[
            Self::Settings,
            Self::Commands,
            Self::RingShortcut,
            Self::Keyboard,
            Self::MouseGesture,
            Self::Gamepad,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OperationSettingsTab {
    Behavior,
    RightDrag,
    MouseButtons,
}

impl OperationSettingsTab {
    fn label(self) -> &'static str {
        match self {
            Self::Behavior => "動作",
            Self::RightDrag => "右ドラッグ・右クリック",
            Self::MouseButtons => "マウスボタン",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OperationAssignmentTab {
    Keyboard,
    RingPad,
    MouseButtons,
    MouseGesture,
}

impl OperationAssignmentTab {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Keyboard => "キー",
            Self::RingPad => "リング/パッド",
            Self::MouseButtons => "マウスボタン",
            Self::MouseGesture => "マウス",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OperationAssignmentTarget {
    Key(crate::keymap::KeyAction),
    Chord(crate::keymap::Chord),
    Ring {
        context: RingShortcutContext,
        action: RingActionId,
    },
    RingSlot {
        context: RingShortcutContext,
        direction: RingDirection,
    },
    MouseButton {
        context: RingShortcutContext,
        slot: MouseButtonSlot,
    },
    MouseGesture {
        context: RightDragContext,
        index: usize,
    },
}

pub(super) fn operation_keyboard_context_filter_label(
    context: Option<crate::keymap::KeyContext>,
) -> &'static str {
    use crate::keymap::KeyContext;
    match context {
        None => "すべて",
        Some(KeyContext::Erase)
        | Some(KeyContext::Conceal)
        | Some(KeyContext::Crop)
        | Some(KeyContext::SnsSplit)
        | Some(KeyContext::Text)
        | Some(KeyContext::LocalAdjust) => "編集モード",
        Some(context) => context.description(),
    }
}

pub(super) fn operation_keyboard_context_filter_matches(
    filter: Option<crate::keymap::KeyContext>,
    context: crate::keymap::KeyContext,
) -> bool {
    use crate::keymap::KeyContext;
    let Some(filter) = filter else {
        return true;
    };
    if matches!(
        filter,
        KeyContext::Erase
            | KeyContext::Conceal
            | KeyContext::Crop
            | KeyContext::SnsSplit
            | KeyContext::Text
            | KeyContext::LocalAdjust
    ) {
        return matches!(
            context,
            KeyContext::Erase
                | KeyContext::Conceal
                | KeyContext::Crop
                | KeyContext::SnsSplit
                | KeyContext::Text
                | KeyContext::LocalAdjust
        );
    }
    context == filter
}

pub(super) fn operation_keyboard_context_filter_for_context(
    context: crate::keymap::KeyContext,
) -> Option<crate::keymap::KeyContext> {
    use crate::keymap::KeyContext;
    Some(match context {
        KeyContext::Erase
        | KeyContext::Conceal
        | KeyContext::Crop
        | KeyContext::SnsSplit
        | KeyContext::Text
        | KeyContext::LocalAdjust => KeyContext::Erase,
        context => context,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OperationAssignmentEditor {
    pub target: OperationAssignmentTarget,
    pub tab: OperationAssignmentTab,
}

#[derive(Clone, Debug)]
pub(crate) struct OperationMouseGestureRecorder {
    pub context: RightDragContext,
    pub action: RingActionId,
    pub replace_index: Option<usize>,
    pub pattern: Vec<MouseGestureDirection>,
    pub points: Vec<egui::Pos2>,
    pub recording: bool,
    pub error: Option<String>,
}

// ── ツリーカテゴリ定義 ──────────────────────────────────────────

struct TreeCategory {
    label: &'static str,
    /// カテゴリ自体がページを持つ場合の直接ページ
    page: Option<PreferencesPage>,
    /// 子ページ（空ならリーフカテゴリ）
    children: &'static [PreferencesPage],
}

const TREE: &[TreeCategory] = &[
    TreeCategory {
        label: "全体設定",
        page: Some(PreferencesPage::General),
        children: &[],
    },
    TreeCategory {
        label: "起動と連携",
        page: None,
        children: &[
            PreferencesPage::Startup,
            PreferencesPage::ExternalTools,
            PreferencesPage::ExplorerIntegration,
            PreferencesPage::TrayResidency,
            PreferencesPage::UpdateCheck,
        ],
    },
    TreeCategory {
        label: "表示",
        page: None,
        children: &[
            PreferencesPage::Font,
            PreferencesPage::Thumbnail,
            PreferencesPage::SpreadMode,
            PreferencesPage::Slideshow,
            PreferencesPage::Capture,
            PreferencesPage::CreativeLut,
            PreferencesPage::MenuLayout,
        ],
    },
    TreeCategory {
        label: "パフォーマンス",
        page: None,
        children: &[
            PreferencesPage::Parallelism,
            PreferencesPage::Prefetch,
            PreferencesPage::GpuMemory,
            PreferencesPage::AiBackend,
            PreferencesPage::Cache,
        ],
    },
    TreeCategory {
        label: "ライブラリ",
        page: None,
        children: &[
            PreferencesPage::Folder,
            PreferencesPage::Book,
            PreferencesPage::PlaybackResume,
            PreferencesPage::Rating,
            PreferencesPage::IndexerSpeed,
        ],
    },
    TreeCategory {
        label: "ファイル処理",
        page: None,
        children: &[
            PreferencesPage::DuplicateFiles,
            PreferencesPage::ExifDisplay,
            PreferencesPage::SusiePlugins,
        ],
    },
    TreeCategory {
        label: "動画・音声",
        page: None,
        children: &[PreferencesPage::Video, PreferencesPage::Vst3],
    },
    TreeCategory {
        label: "拡張と診断",
        page: None,
        children: &[PreferencesPage::EditingAddon, PreferencesPage::Developer],
    },
];

// ── 一時編集状態 ────────────────────────────────────────────────

enum CreativeLutTransactionState {
    Editing {
        created: HashSet<uuid::Uuid>,
        removed: HashSet<uuid::Uuid>,
    },
    Committed,
}

struct CreativeLutTransaction {
    state: CreativeLutTransactionState,
}

impl Default for CreativeLutTransaction {
    fn default() -> Self {
        Self {
            state: CreativeLutTransactionState::Editing {
                created: HashSet::new(),
                removed: HashSet::new(),
            },
        }
    }
}

impl CreativeLutTransaction {
    fn reserve_created(&mut self, id: uuid::Uuid) {
        if let CreativeLutTransactionState::Editing { created, .. } = &mut self.state {
            created.insert(id);
        }
    }

    fn forget_created(&mut self, id: uuid::Uuid) {
        if let CreativeLutTransactionState::Editing { created, .. } = &mut self.state {
            created.remove(&id);
        }
    }

    fn mark_removed(&mut self, id: uuid::Uuid) {
        if let CreativeLutTransactionState::Editing { removed, .. } = &mut self.state {
            removed.insert(id);
        }
    }

    fn commit(&mut self) {
        let CreativeLutTransactionState::Editing { removed, .. } =
            std::mem::replace(&mut self.state, CreativeLutTransactionState::Committed)
        else {
            return;
        };
        crate::creative_lut::remove_managed_creative_luts_async(removed.into_iter().collect());
    }
}

impl Drop for CreativeLutTransaction {
    fn drop(&mut self) {
        if let CreativeLutTransactionState::Editing { created, .. } = &self.state {
            crate::creative_lut::remove_managed_creative_luts_async(
                created.iter().copied().collect(),
            );
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExternalToolAddSource {
    Executable,
    Associated,
}

#[derive(Clone, Debug)]
pub(super) enum ExternalToolPathStatus {
    Unchecked,
    Checking,
    Valid,
    Invalid,
    Error(String),
}

pub(super) struct ExternalToolHandlersPending {
    cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<Vec<crate::open_with::AppHandler>>,
}

impl Drop for ExternalToolHandlersPending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ExternalToolLaunchCandidate {
    pub display_name: String,
    pub launch: crate::external_tool::ExternalToolLaunch,
}

fn external_tool_launch_candidates(
    handlers: &[crate::open_with::AppHandler],
) -> Vec<ExternalToolLaunchCandidate> {
    let mut candidates: Vec<ExternalToolLaunchCandidate> = Vec::new();
    for handler in handlers {
        let launch = crate::external_tool::ExternalToolLaunch::Association {
            handler_id: handler.handler_id.clone(),
        };
        if candidates
            .iter()
            .any(|candidate| candidate.launch.same_target(&launch))
        {
            continue;
        }
        candidates.push(ExternalToolLaunchCandidate {
            display_name: handler.display_name.clone(),
            launch,
        });
    }
    candidates
}

pub(super) struct ExternalToolPathCheckResult {
    pub tool_id: crate::external_tool::ExternalToolId,
    pub executable: Option<Result<bool, String>>,
    pub working_directory: Option<Result<bool, String>>,
}

pub(super) struct ExternalToolPathCheckPending {
    cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<ExternalToolPathCheckResult>,
}

impl Drop for ExternalToolPathCheckPending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(crate) struct PreferencesState {
    /// 編集用の Settings 一時コピー
    pub settings: Settings,
    /// 現在選択中のページ
    pub selected: PreferencesPage,
    /// 右ペインのスクロール状態をページ切替ごとに新しくする世代。
    /// 同じページの再描画では維持し、別ページへ移ったときだけ増やす。
    pub right_panel_scroll_generation: u64,
    /// 左ツリー上部の環境設定検索文字列。
    pub search_query: String,
    /// 右ペインを設定ページではなく検索結果一覧へ切り替える。
    pub showing_results: bool,
    /// 検索結果から遷移した直後にスクロールするページ内 anchor。
    pub pending_anchor: Option<&'static str>,
    /// 現在強調表示している anchor と表示開始時刻 (egui input time)。
    pub highlight: Option<(&'static str, f64)>,
    /// 展開中のカテゴリラベル
    pub expanded: HashSet<&'static str>,

    // ── 外部ツールページ ────────────────────────────────────────
    /// 環境設定を開いた時点の現在項目。P1 のプレビュー / 試験起動は実ファイルだけを受ける。
    external_tool_target: crate::external_tool::LaunchTarget,
    external_tool_selected: Option<crate::external_tool::ExternalToolId>,
    external_tool_editor_loaded_for: Option<crate::external_tool::ExternalToolId>,
    external_tool_executable_input: String,
    external_tool_working_directory_input: String,
    external_tool_add_source: Option<ExternalToolAddSource>,
    external_tool_candidates: Vec<ExternalToolLaunchCandidate>,
    external_tool_handlers_pending: Option<ExternalToolHandlersPending>,
    external_tool_handlers_for_editing: bool,
    external_tool_path_check_pending: Option<ExternalToolPathCheckPending>,
    external_tool_path_check_due: Option<(
        std::time::Instant,
        crate::external_tool::ExternalToolId,
        Option<PathBuf>,
        Option<PathBuf>,
    )>,
    external_tool_executable_status: ExternalToolPathStatus,
    external_tool_working_directory_status: ExternalToolPathStatus,
    external_tool_message: Option<String>,
    external_tool_launch_requested: Option<(
        crate::external_tool::ExternalTool,
        crate::external_tool::LaunchTarget,
    )>,

    // ── UI フォント設定 ──────────────────────────────────────────
    /// システムフォント列挙結果。走査は Font ページ初回表示時に worker で開始する。
    pub ui_font_catalog: Vec<crate::ui_font_catalog::UiFontFace>,
    pub ui_font_filter: String,
    pub ui_font_catalog_rx:
        Option<std::sync::mpsc::Receiver<Result<Vec<crate::ui_font_catalog::UiFontFace>, String>>>,
    pub ui_font_catalog_started: bool,
    pub ui_font_import_rx:
        Option<std::sync::mpsc::Receiver<Result<Vec<crate::ui_font_catalog::UiFontFace>, String>>>,
    pub ui_font_message: Option<String>,
    pub ui_font_initial: crate::settings::UiFontSettings,
    pub ui_font_preview_rx:
        Option<std::sync::mpsc::Receiver<(String, Result<egui::ColorImage, String>)>>,
    pub ui_font_preview_texture: Option<egui::TextureHandle>,
    pub ui_font_preview_ready_key: Option<String>,
    pub ui_font_preview_requested_at: Option<std::time::Instant>,
    pub ui_font_preview_error: Option<String>,
    /// `.cube` の検証は UI スレッド外で行う。
    pub creative_lut_import_rx: Option<
        std::sync::mpsc::Receiver<(
            uuid::Uuid,
            Result<crate::creative_lut::CreativeLutImport, String>,
        )>,
    >,
    pub creative_lut_message: Option<String>,
    creative_lut_transaction: CreativeLutTransaction,

    // ページ固有の一時状態
    pub manual_threads: usize,
    pub capture_output_dir_input: String,
    pub book_root_input: String,
    pub startup_folder_path_input: String,
    pub exif_add_tag_input: String,
    pub command_filter: String,
    pub command_key_filter: String,
    pub command_selected: Option<crate::keymap::KeyAction>,
    pub command_edit_loaded_for: Option<crate::keymap::KeyAction>,
    pub command_chord_inputs: [String; 3],
    pub command_capture_slot: Option<usize>,
    pub command_edit_error: Option<String>,
    pub command_edit_notice: Option<String>,
    pub command_editor_source_chord: Option<crate::keymap::Chord>,
    pub operation_tab: OperationCustomizeTab,
    pub operation_settings_tab: OperationSettingsTab,
    pub operation_assignment_editor: Option<OperationAssignmentEditor>,
    /// コマンド一覧 / キーボード図を絞り込む、利用者が選んだ場所。
    pub operation_keyboard_context: Option<crate::keymap::KeyContext>,
    /// 開いている割り当て編集セッションだけが使う場所。Key target では action から導出する。
    pub operation_assignment_keyboard_context: Option<crate::keymap::KeyContext>,
    pub operation_keyboard_ctrl: bool,
    pub operation_keyboard_shift: bool,
    pub operation_keyboard_alt: bool,
    pub operation_ring_context: RingShortcutContext,
    pub operation_gamepad_context: RingShortcutContext,
    pub operation_mouse_gesture_context: RightDragContext,
    pub operation_mouse_gesture_inputs: HashMap<(RightDragContext, usize), String>,
    pub operation_mouse_gesture_recorder: Option<OperationMouseGestureRecorder>,
    /// EXIF タグ設定で折りたたみ中のグループ。`HashSet` に入っているものが折りたたみ。
    pub exif_collapsed_groups: HashSet<crate::exif_reader::TagGroup>,
    /// カスタム追加直後に「自動スクロールして見せる」タグ名 (1 フレームだけ持つ)。
    pub exif_scroll_to_added: Option<String>,

    // 初回に1度だけ取得するキャッシュ値
    pub auto_thread_count: usize,
    pub vram_mib: Option<u64>,

    // ── 動画ページ用の一時状態 ─────────────────────────────────
    /// `audio_normalize.db` を開けているか。開けていない場合は削除ボタンを出さない。
    pub audio_normalize_db_available: bool,
    /// 環境設定を開いた時点 / 削除後の音量ノーマライズ測定値件数。
    pub audio_normalize_entry_count: usize,
    /// 音量ノーマライズ測定値削除の確認ダイアログ表示中フラグ。
    pub audio_normalize_clear_confirm_open: bool,
    /// 確認ダイアログで削除が確定されたことを App 側へ伝える one-shot フラグ。
    pub audio_normalize_clear_requested: bool,
    /// 直近の音量ノーマライズ測定値削除結果。
    pub audio_normalize_clear_result: Option<String>,

    // ── 履歴と復元ページ用 ─────────────────────────────────────
    /// 環境設定を開いた時点 / 削除後の ZIP/PDF 読書位置の記憶件数。
    pub book_resume_entry_count: usize,
    /// ZIP/PDF 読書位置クリアを App 側へ伝える one-shot フラグ。
    pub book_resume_clear_requested: bool,
    /// 直近の ZIP/PDF 読書位置削除結果。
    pub book_resume_clear_result: Option<String>,
    /// 環境設定を開いた時点 / 削除後の閲覧履歴の記憶件数。
    pub reading_history_entry_count: usize,
    /// 閲覧履歴クリアを App 側へ伝える one-shot フラグ。
    pub reading_history_clear_requested: bool,
    /// 直近の閲覧履歴削除結果。
    pub reading_history_clear_result: Option<String>,

    // ── エクスプローラ連携ページ用 ──────────────────────────────
    /// SendTo ショートカットの状態。ページを初めて開いた時と操作後に更新する。
    pub send_to_status: Option<Result<crate::explorer_integration::SendToShortcutStatus, String>>,
    /// SendTo 登録/削除ボタンの直近メッセージ。
    pub send_to_action_message: Option<String>,

    // ── AI バックエンド ページ用のキャッシュ ────────────────────
    /// プライマリ GPU のベンダー (NVIDIA でなければ TRT は disabled に)
    pub gpu_vendor: Option<crate::gpu_info::GpuVendor>,
    /// TRT ワーカープールが現在 attach されているか (Phase 3、ホットリロード)。
    /// `true` ならアップスケール/デノイズはワーカー経由 TRT で動いている。
    pub trt_worker_active: bool,
    /// 起動時にフォールバックが起きた場合の理由 (UI バナー表示用)
    pub current_runtime_fallback_reason: Option<String>,
    /// TRT pack インストール済みか
    pub trt_pack_installed: bool,
    /// TRT pack の総ディスク使用量 (MiB)
    pub trt_pack_size_mib: u64,
    /// TRT engine cache の総ディスク使用量 (MiB)
    pub trt_engine_cache_size_mib: u64,
    /// 「TensorRT パックをダウンロード」ボタンが押されたか。
    /// 環境設定の Apply/Cancel 後に App 側で読み取って TRT install dialog を開く。
    pub start_trt_install_requested: bool,
    /// エンジンキャッシュ削除の確認ダイアログ表示中フラグ (Codex P3-2)。
    pub trt_cache_delete_confirm_open: bool,
    /// 「TensorRT パックを削除」ボタンで実行された pack 全体削除をリクエスト。
    /// dialog 内では state 更新だけ行い、実際の worker pool detach + ファイル削除は
    /// App 側で Preferences ウィンドウ closure 抜けた後に処理する
    /// (= worker pool が DLL を握ったままだと remove_dir_all が失敗するため)。
    pub uninstall_trt_pack_requested: bool,

    // ── 編集用追加パック ページ用のキャッシュ ────────────────────
    /// 編集用追加パックの導入状態 (環境設定ダイアログを開いた時点のスナップショット)。
    pub editing_addon_status: crate::editing_addon::AddonStatus,
    /// 導入済み pack のディスク使用量 (MiB)。未導入なら 0。
    pub editing_addon_size_mib: u64,
    /// 導入済み pack のフォント数。
    pub editing_addon_font_count: usize,
    /// 被写体分離モデル名 (manifest 由来、表示用)。
    pub editing_addon_subject_model: String,
    /// 「ダウンロード」/「更新・再ダウンロード」ボタンが押されたか。
    /// Apply/Cancel 後に App 側で読み取って editing_addon install dialog を開く。
    pub start_editing_addon_install_requested: bool,
    /// 「削除」確認ダイアログ表示中フラグ。
    pub editing_addon_delete_confirm_open: bool,
    /// pack 削除をリクエスト。dialog closure 抜けた後に App 側で背景削除する。
    pub uninstall_editing_addon_requested: bool,
    /// インストール先フォルダを開くリクエスト。
    pub open_editing_addon_folder_requested: bool,

    // ── VST3 プラグイン編集 ────────────────────────────────────────
    /// 環境設定を開いた時点でスキャンされていた VST3 プラグイン候補のスナップショット。
    /// VST3 ページ内でスキャン / 再スキャンで更新する。Apply 後に App 側に反映する。
    #[cfg(windows)]
    pub vst3_discovered: Vec<crate::video::dsp::DiscoveredPlugin>,
    /// VST3 scan/probe worker の完了通知。bridge subprocess で bus probe するため
    /// UI thread では直接走らせない。
    #[cfg(windows)]
    pub vst3_scan_rx: Option<std::sync::mpsc::Receiver<Vst3ScanMessage>>,
    #[cfg(windows)]
    pub vst3_scan_in_progress: bool,
    #[cfg(windows)]
    pub vst3_scan_error: Option<String>,
    #[cfg(windows)]
    pub vst3_scan_done: usize,
    #[cfg(windows)]
    pub vst3_scan_total: usize,
    #[cfg(windows)]
    pub vst3_scan_current: String,
    /// VST3 ページ内のフィルタ文字列。
    pub vst3_filter: String,
    /// 音声入力を持たない plugin (= Instrument / MIDI FX 等) も候補一覧に表示する。
    #[cfg(windows)]
    pub vst3_show_unusable: bool,
    /// 現在 auto-bypass されているスロットのスナップショット (= 名前, latency_ms)。
    /// VST3 ページ下部の赤字警告表示用。`show_preferences_dialog` の頭で
    /// `dsp_bridge` から毎フレーム refresh される (= ON/OFF が即座に反映)。
    #[cfg(windows)]
    pub vst3_auto_bypassed: Vec<(String, f64)>,

    /// 開発者タブ「ログを zip にする」ボタンの直近の実行結果。
    /// `Ok(path)` なら作成した zip パス、`Err(msg)` なら失敗理由を表示する。
    pub diag_export_result: Option<Result<std::path::PathBuf, String>>,
}

impl PreferencesState {
    pub(super) fn select_external_tool(
        &mut self,
        selected: Option<crate::external_tool::ExternalToolId>,
    ) {
        self.external_tool_selected = selected;
        self.external_tool_editor_loaded_for = None;
        self.external_tool_message = None;
    }

    pub(super) fn add_external_tool(&mut self, mut tool: crate::external_tool::ExternalTool) {
        // 新規ツールは既定 ON。ただし ON の登録が既に 10 件を超えている場合だけ
        // メニューを伸ばさないよう OFF で追加する。複製も新規登録として同じ既定を使う。
        tool.show_in_context_menu =
            crate::external_tool::show_in_context_menu_by_default(&self.settings.external_tools);
        tool.id = crate::external_tool::next_id(&self.settings.external_tools);
        let id = tool.id;
        self.settings.external_tools.push(tool);
        self.select_external_tool(Some(id));
    }

    pub(super) fn ensure_external_tool_editor_loaded(&mut self) {
        let Some(id) = self.external_tool_selected else {
            self.external_tool_editor_loaded_for = None;
            return;
        };
        if self.external_tool_editor_loaded_for == Some(id) {
            return;
        }
        let Some(tool) = self
            .settings
            .external_tools
            .iter()
            .find(|tool| tool.id == id)
        else {
            self.select_external_tool(None);
            return;
        };
        self.external_tool_executable_input = tool
            .launch
            .executable()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        self.external_tool_working_directory_input = tool
            .working_directory
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        self.external_tool_editor_loaded_for = Some(id);
        self.schedule_external_tool_path_check(std::time::Duration::ZERO);
    }

    pub(super) fn schedule_external_tool_path_check(&mut self, delay: std::time::Duration) {
        let Some(id) = self.external_tool_selected else {
            return;
        };
        let Some(tool) = self
            .settings
            .external_tools
            .iter()
            .find(|tool| tool.id == id)
        else {
            return;
        };
        self.external_tool_path_check_due = Some((
            std::time::Instant::now() + delay,
            id,
            tool.launch.executable().map(Path::to_path_buf),
            tool.launch
                .uses_process_options()
                .then(|| tool.working_directory.clone())
                .flatten(),
        ));
        self.external_tool_executable_status = if tool.launch.uses_process_options() {
            ExternalToolPathStatus::Checking
        } else {
            ExternalToolPathStatus::Unchecked
        };
        self.external_tool_working_directory_status =
            if tool.launch.uses_process_options() && tool.working_directory.is_some() {
                ExternalToolPathStatus::Checking
            } else {
                ExternalToolPathStatus::Unchecked
            };
    }

    pub(super) fn start_external_tool_handler_enumeration(
        &mut self,
        for_editing: bool,
        ctx: &egui::Context,
    ) {
        self.external_tool_handlers_pending = None;
        self.external_tool_candidates.clear();
        self.external_tool_handlers_for_editing = for_editing;
        let extension = self
            .external_tool_target
            .real_file()
            .ok()
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{}", extension.to_ascii_lowercase()));
        let Some(extension) = extension else {
            self.external_tool_message =
                Some("関連付けを調べるには、拡張子のある実ファイルを選択してください".to_string());
            return;
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        match std::thread::Builder::new()
            .name("external-tool-handlers".to_string())
            .spawn(move || {
                if cancel_worker.load(Ordering::Relaxed) {
                    return;
                }
                let handlers = crate::open_with::enumerate_handlers(&extension);
                if !cancel_worker.load(Ordering::Relaxed) {
                    let _ = tx.send(handlers);
                    repaint.request_repaint();
                }
            }) {
            Ok(_) => {
                self.external_tool_handlers_pending =
                    Some(ExternalToolHandlersPending { cancel, rx });
                self.external_tool_message = None;
            }
            Err(error) => {
                self.external_tool_message =
                    Some(format!("関連付けアプリの列挙を開始できません: {error}"));
            }
        }
    }

    fn start_external_tool_path_check(
        &mut self,
        tool_id: crate::external_tool::ExternalToolId,
        executable: Option<PathBuf>,
        working_directory: Option<PathBuf>,
        ctx: &egui::Context,
    ) {
        self.external_tool_path_check_pending = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        match std::thread::Builder::new()
            .name("external-tool-path-check".to_string())
            .spawn(move || {
                fn check(path: &std::path::Path, expected_file: bool) -> Result<bool, String> {
                    match std::fs::metadata(path) {
                        Ok(metadata) => Ok(if expected_file {
                            metadata.is_file()
                        } else {
                            metadata.is_dir()
                        }),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                        Err(error) => Err(error.to_string()),
                    }
                }
                if cancel_worker.load(Ordering::Relaxed) {
                    return;
                }
                let executable = executable.as_deref().map(|path| check(path, true));
                if cancel_worker.load(Ordering::Relaxed) {
                    return;
                }
                let working_directory = working_directory.as_deref().map(|path| check(path, false));
                if !cancel_worker.load(Ordering::Relaxed) {
                    let _ = tx.send(ExternalToolPathCheckResult {
                        tool_id,
                        executable,
                        working_directory,
                    });
                    repaint.request_repaint();
                }
            }) {
            Ok(_) => {
                self.external_tool_path_check_pending =
                    Some(ExternalToolPathCheckPending { cancel, rx });
            }
            Err(error) => {
                self.external_tool_executable_status = ExternalToolPathStatus::Error(format!(
                    "パス確認 worker を開始できません: {error}"
                ));
                self.external_tool_working_directory_status =
                    self.external_tool_executable_status.clone();
            }
        }
    }

    pub(super) fn poll_external_tool_workers(&mut self, ctx: &egui::Context) {
        if self
            .external_tool_path_check_due
            .as_ref()
            .is_some_and(|(due, ..)| std::time::Instant::now() >= *due)
        {
            let (_, id, executable, directory) = self.external_tool_path_check_due.take().unwrap();
            self.start_external_tool_path_check(id, executable, directory, ctx);
        }

        let handlers_result = self
            .external_tool_handlers_pending
            .as_ref()
            .and_then(|pending| match pending.rx.try_recv() {
                Ok(handlers) => Some(Some(handlers)),
                Err(mpsc::TryRecvError::Disconnected) => Some(None),
                Err(mpsc::TryRecvError::Empty) => None,
            });
        if let Some(result) = handlers_result {
            self.external_tool_handlers_pending = None;
            if let Some(handlers) = result {
                self.external_tool_candidates = external_tool_launch_candidates(&handlers);
                if self.external_tool_candidates.is_empty() {
                    self.external_tool_message =
                        Some("関連付けアプリが見つかりませんでした".to_string());
                }
            }
        }

        let path_result = self
            .external_tool_path_check_pending
            .as_ref()
            .and_then(|pending| match pending.rx.try_recv() {
                Ok(result) => Some(Some(result)),
                Err(mpsc::TryRecvError::Disconnected) => Some(None),
                Err(mpsc::TryRecvError::Empty) => None,
            });
        if let Some(result) = path_result {
            self.external_tool_path_check_pending = None;
            if let Some(result) = result
                && self.external_tool_selected == Some(result.tool_id)
            {
                self.external_tool_executable_status = path_status(result.executable);
                self.external_tool_working_directory_status = path_status(result.working_directory);
            }
        }

        if self.external_tool_handlers_pending.is_some()
            || self.external_tool_path_check_pending.is_some()
            || self.external_tool_path_check_due.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    pub(crate) fn from_settings(
        s: &Settings,
        external_tool_target: crate::external_tool::LaunchTarget,
        ai_runtime: Option<&crate::ai::runtime::AiRuntime>,
        audio_normalize_db_available: bool,
        audio_normalize_entry_count: usize,
        book_resume_entry_count: usize,
        reading_history_entry_count: usize,
    ) -> Self {
        let manual_threads = match &s.parallelism {
            Parallelism::Manual(n) => *n,
            Parallelism::Auto => s.parallelism.thread_count(),
        };
        let auto_thread_count = {
            let cores = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2);
            (cores / 2).max(1)
        };
        let expanded = HashSet::new();

        // AI バックエンドページ用の情報を 1 回だけ取得 (環境設定ダイアログを開いた時点)
        let gpu_vendor = crate::gpu_info::query_primary_gpu_vendor();
        let (current_runtime_fallback_reason, trt_worker_active) = match ai_runtime {
            Some(rt) => (
                rt.active_backend().fallback_reason.clone(),
                rt.has_worker_pool(),
            ),
            None => (None, false),
        };
        let trt_pack_installed = crate::ai::tensorrt_pack::is_pack_installed();
        let trt_pack_size_mib = if trt_pack_installed {
            dir_size_bytes(&crate::ai::tensorrt_pack::pack_dir()) / (1024 * 1024)
        } else {
            0
        };
        let trt_engine_cache_size_mib =
            dir_size_bytes(&crate::ai::tensorrt_pack::engine_cache_dir()) / (1024 * 1024);

        // 編集用追加パックの状態を 1 回だけ取得。size / フォント数 / モデル名はすべて
        // manifest 1 回読みで賄い、ディレクトリ再帰走査 (dir_size_bytes) や read_dir
        // (installed_fonts) といった UI スレッド同期 I/O を避ける (Codex P3)。
        // 表示サイズは manifest の per-file bytes 合計 (= 展開後サイズ概算)。
        let editing_addon_status = crate::editing_addon::addon_status();
        let (editing_addon_size_mib, editing_addon_font_count, editing_addon_subject_model) =
            if let crate::editing_addon::AddonStatus::Valid { version } = &editing_addon_status {
                crate::editing_addon::read_pack_manifest(version)
                    .map(|m| {
                        let size = m.total_bytes() / (1024 * 1024);
                        let fonts = m.fonts().count();
                        let model = m
                            .subject_matte_model()
                            .map(|f| f.model_id.clone().unwrap_or_else(|| f.path.clone()))
                            .unwrap_or_default();
                        (size, fonts, model)
                    })
                    .unwrap_or((0, 0, String::new()))
            } else {
                (0, 0, String::new())
            };

        Self {
            settings: s.clone(),
            selected: PreferencesPage::General,
            right_panel_scroll_generation: 0,
            search_query: String::new(),
            showing_results: false,
            pending_anchor: None,
            highlight: None,
            expanded,
            external_tool_target,
            external_tool_selected: s.external_tools.first().map(|tool| tool.id),
            external_tool_editor_loaded_for: None,
            external_tool_executable_input: String::new(),
            external_tool_working_directory_input: String::new(),
            external_tool_add_source: None,
            external_tool_candidates: Vec::new(),
            external_tool_handlers_pending: None,
            external_tool_handlers_for_editing: false,
            external_tool_path_check_pending: None,
            external_tool_path_check_due: None,
            external_tool_executable_status: ExternalToolPathStatus::Unchecked,
            external_tool_working_directory_status: ExternalToolPathStatus::Unchecked,
            external_tool_message: None,
            external_tool_launch_requested: None,
            ui_font_catalog: Vec::new(),
            ui_font_filter: String::new(),
            ui_font_catalog_rx: None,
            ui_font_catalog_started: false,
            ui_font_import_rx: None,
            ui_font_message: None,
            ui_font_initial: s.ui_font.clone(),
            ui_font_preview_rx: None,
            ui_font_preview_texture: None,
            ui_font_preview_ready_key: None,
            ui_font_preview_requested_at: None,
            ui_font_preview_error: None,
            creative_lut_import_rx: None,
            creative_lut_message: None,
            creative_lut_transaction: CreativeLutTransaction::default(),
            manual_threads,
            capture_output_dir_input: s
                .capture_output_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            book_root_input: s
                .book_root
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            startup_folder_path_input: s
                .startup_folder_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            exif_add_tag_input: String::new(),
            command_filter: String::new(),
            command_key_filter: String::new(),
            command_selected: None,
            command_edit_loaded_for: None,
            command_chord_inputs: std::array::from_fn(|_| String::new()),
            command_capture_slot: None,
            command_edit_error: None,
            command_edit_notice: None,
            command_editor_source_chord: None,
            operation_tab: OperationCustomizeTab::Settings,
            operation_settings_tab: OperationSettingsTab::Behavior,
            operation_assignment_editor: None,
            operation_keyboard_context: None,
            operation_assignment_keyboard_context: None,
            operation_keyboard_ctrl: false,
            operation_keyboard_shift: false,
            operation_keyboard_alt: false,
            operation_ring_context: RingShortcutContext::Grid,
            operation_gamepad_context: RingShortcutContext::Grid,
            operation_mouse_gesture_context: RightDragContext::Grid,
            operation_mouse_gesture_inputs: HashMap::new(),
            operation_mouse_gesture_recorder: None,
            exif_collapsed_groups: HashSet::new(),
            exif_scroll_to_added: None,
            auto_thread_count,
            vram_mib: crate::gpu_info::query_vram_summary_mib(),
            audio_normalize_db_available,
            audio_normalize_entry_count,
            audio_normalize_clear_confirm_open: false,
            audio_normalize_clear_requested: false,
            audio_normalize_clear_result: None,
            book_resume_entry_count,
            book_resume_clear_requested: false,
            book_resume_clear_result: None,
            reading_history_entry_count,
            reading_history_clear_requested: false,
            reading_history_clear_result: None,
            send_to_status: None,
            send_to_action_message: None,
            gpu_vendor,
            trt_worker_active,
            current_runtime_fallback_reason,
            trt_pack_installed,
            trt_pack_size_mib,
            trt_engine_cache_size_mib,
            start_trt_install_requested: false,
            trt_cache_delete_confirm_open: false,
            uninstall_trt_pack_requested: false,
            editing_addon_status,
            editing_addon_size_mib,
            editing_addon_font_count,
            editing_addon_subject_model,
            start_editing_addon_install_requested: false,
            editing_addon_delete_confirm_open: false,
            uninstall_editing_addon_requested: false,
            open_editing_addon_folder_requested: false,
            #[cfg(windows)]
            vst3_discovered: Vec::new(),
            #[cfg(windows)]
            vst3_scan_rx: None,
            #[cfg(windows)]
            vst3_scan_in_progress: false,
            #[cfg(windows)]
            vst3_scan_error: None,
            #[cfg(windows)]
            vst3_scan_done: 0,
            #[cfg(windows)]
            vst3_scan_total: 0,
            #[cfg(windows)]
            vst3_scan_current: String::new(),
            vst3_filter: String::new(),
            #[cfg(windows)]
            vst3_show_unusable: false,
            #[cfg(windows)]
            vst3_auto_bypassed: Vec::new(),
            diag_export_result: None,
        }
    }

    pub(super) fn ensure_ui_font_tasks_started(&mut self, ctx: &egui::Context) {
        if !self.ui_font_catalog_started {
            self.ui_font_catalog_started = true;
            let (tx, rx) = std::sync::mpsc::channel();
            self.ui_font_catalog_rx = Some(rx);
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("ui-font-catalog".to_string())
                .spawn(move || {
                    let _ = tx.send(crate::ui_font_catalog::enumerate_ui_fonts());
                    ctx.request_repaint();
                });
            if let Err(err) = spawned {
                self.ui_font_catalog_rx = None;
                self.ui_font_message = Some(format!("フォント一覧を開始できませんでした: {err}"));
            }
        }
        if self.ui_font_preview_ready_key.is_none()
            && self.ui_font_preview_rx.is_none()
            && self.ui_font_preview_requested_at.is_none()
        {
            self.ui_font_preview_requested_at = Some(
                std::time::Instant::now()
                    .checked_sub(std::time::Duration::from_millis(200))
                    .unwrap_or_else(std::time::Instant::now),
            );
        }
    }

    pub(super) fn mark_ui_font_changed(&mut self, ctx: &egui::Context) {
        self.settings.ui_font.sanitize();
        self.ui_font_preview_requested_at = Some(std::time::Instant::now());
        self.ui_font_preview_error = None;
        ctx.request_repaint_after(std::time::Duration::from_millis(160));
    }

    pub(super) fn start_ui_font_import(&mut self, path: std::path::PathBuf, ctx: &egui::Context) {
        if self.ui_font_import_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.ui_font_import_rx = Some(rx);
        self.ui_font_message = Some("フォントを取り込んでいます…".to_string());
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("ui-font-import".to_string())
            .spawn(move || {
                let _ = tx.send(crate::ui_font_catalog::import_ui_font(&path));
                ctx.request_repaint();
            });
        if let Err(err) = spawned {
            self.ui_font_import_rx = None;
            self.ui_font_message = Some(format!("フォント追加を開始できませんでした: {err}"));
        }
    }

    pub(super) fn poll_ui_font_tasks(&mut self, ctx: &egui::Context) {
        if let Some(rx) = self.ui_font_catalog_rx.as_ref()
            && let Ok(result) = rx.try_recv()
        {
            self.ui_font_catalog_rx = None;
            match result {
                Ok(faces) => {
                    self.ui_font_catalog = faces;
                    if matches!(
                        self.settings.ui_font.selection,
                        crate::settings::UiFontSelection::Face { .. }
                    ) {
                        let canonical = self
                            .ui_font_catalog
                            .iter()
                            .find(|face| {
                                face.selection
                                    .same_source_face(&self.settings.ui_font.selection)
                            })
                            .map(|face| face.selection.clone());
                        if let Some(canonical) = canonical {
                            // ラベル改善前の保存値でも path + face index が同じなら有効。
                            // 説明用ラベルだけを更新し、フォント再構築や未保存扱いにはしない。
                            if canonical != self.settings.ui_font.selection {
                                self.settings.ui_font.selection = canonical.clone();
                                if canonical.same_source_face(&self.ui_font_initial.selection) {
                                    self.ui_font_initial.selection = canonical;
                                }
                            }
                        } else {
                            self.settings.ui_font.selection =
                                crate::settings::UiFontSelection::Default;
                            self.settings.ui_font.vertical_adjust = 0.0;
                            self.ui_font_message = Some(
                                "保存されていたフォントは日本語の通常書体として利用できないため、既定へ戻しました。"
                                    .to_string(),
                            );
                            self.mark_ui_font_changed(ctx);
                        }
                    }
                }
                Err(err) => self.ui_font_message = Some(err),
            }
        }

        if let Some(rx) = self.ui_font_import_rx.as_ref()
            && let Ok(result) = rx.try_recv()
        {
            self.ui_font_import_rx = None;
            match result {
                Ok(mut faces) => {
                    if let Some(first) = faces.first() {
                        self.settings.ui_font.selection = first.selection.clone();
                        self.settings.ui_font.vertical_adjust = 0.0;
                    }
                    for face in faces.drain(..) {
                        let exists = self
                            .ui_font_catalog
                            .iter()
                            .any(|existing| existing.selection.same_source_face(&face.selection));
                        if !exists {
                            self.ui_font_catalog.push(face);
                        }
                    }
                    crate::ui_font_catalog::sort_ui_font_faces(&mut self.ui_font_catalog);
                    self.ui_font_message = Some("フォントを追加しました。".to_string());
                    self.mark_ui_font_changed(ctx);
                }
                Err(err) => self.ui_font_message = Some(err),
            }
        }

        if let Some(rx) = self.ui_font_preview_rx.as_ref()
            && let Ok((key, result)) = rx.try_recv()
        {
            self.ui_font_preview_rx = None;
            if key == ui_font_settings_key(&self.settings.ui_font) {
                match result {
                    Ok(image) => {
                        self.ui_font_preview_texture = Some(ctx.load_texture(
                            "ui-font-preview",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                        self.ui_font_preview_ready_key = Some(key);
                        self.ui_font_preview_error = None;
                    }
                    Err(err) => {
                        self.ui_font_preview_ready_key = None;
                        self.ui_font_preview_error = Some(err);
                    }
                }
            }
        }

        let current_key = ui_font_settings_key(&self.settings.ui_font);
        let needs_preview = self.ui_font_preview_ready_key.as_deref() != Some(&current_key);
        let debounce_elapsed = self
            .ui_font_preview_requested_at
            .is_some_and(|at| at.elapsed() >= std::time::Duration::from_millis(150));
        if needs_preview && debounce_elapsed && self.ui_font_preview_rx.is_none() {
            self.ui_font_preview_requested_at = None;
            let settings = self.settings.ui_font.clone();
            let key = current_key;
            let (tx, rx) = std::sync::mpsc::channel();
            self.ui_font_preview_rx = Some(rx);
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("ui-font-preview".to_string())
                .spawn(move || {
                    let result = crate::ui_font_catalog::render_preview(&settings);
                    if result.is_ok() {
                        crate::ui_fonts::prepare_fonts(&settings);
                    }
                    let _ = tx.send((key, result));
                    ctx.request_repaint();
                });
            if let Err(err) = spawned {
                self.ui_font_preview_rx = None;
                self.ui_font_preview_error =
                    Some(format!("フォントの準備を開始できませんでした: {err}"));
            }
        }
    }

    pub(super) fn ui_font_apply_ready(&self) -> bool {
        if self.settings.ui_font == self.ui_font_initial {
            return true;
        }
        self.ui_font_preview_error.is_none()
            && self.ui_font_preview_ready_key.as_deref()
                == Some(ui_font_settings_key(&self.settings.ui_font).as_str())
    }

    pub(super) fn start_creative_lut_import(
        &mut self,
        path: std::path::PathBuf,
        ctx: &egui::Context,
    ) {
        if self.creative_lut_import_rx.is_some() {
            return;
        }
        if self
            .settings
            .creative_luts
            .iter()
            .any(|entry| !entry.is_builtin() && entry.path == path)
        {
            self.creative_lut_message =
                Some("このLUTファイルはすでに登録されています。".to_string());
            return;
        }

        let id = uuid::Uuid::new_v4();
        let (tx, rx) = std::sync::mpsc::channel();
        self.creative_lut_import_rx = Some(rx);
        self.creative_lut_transaction.reserve_created(id);
        self.creative_lut_message = Some("LUTを確認してアプリ内へコピーしています…".to_string());
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("creative-lut-import".to_string())
            .spawn(move || {
                let result = crate::creative_lut::import_managed_cube_file(&path, id);
                if let Err(error) = tx.send((id, result))
                    && error.0.1.is_ok()
                {
                    let _ = crate::creative_lut::remove_managed_creative_lut(id);
                }
                repaint.request_repaint();
            });
        if let Err(error) = spawned {
            self.creative_lut_import_rx = None;
            self.creative_lut_transaction.forget_created(id);
            self.creative_lut_message = Some(format!("LUTの確認を開始できませんでした: {error}"));
        }
    }

    pub(super) fn poll_creative_lut_import(&mut self) {
        let Some(rx) = self.creative_lut_import_rx.as_ref() else {
            return;
        };
        let Ok(result) = rx.try_recv() else {
            return;
        };
        self.creative_lut_import_rx = None;
        let (reserved_id, result) = result;
        match result {
            Ok(import) => {
                if self
                    .settings
                    .creative_luts
                    .iter()
                    .any(|entry| !entry.is_builtin() && entry.path == import.entry.path)
                {
                    self.creative_lut_transaction.mark_removed(import.entry.id);
                    self.creative_lut_message =
                        Some("このLUTファイルはすでに登録されています。".to_string());
                } else {
                    let name = import.entry.name.clone();
                    self.settings.creative_luts.push(import.entry);
                    self.creative_lut_message =
                        Some(format!("「{name}」をアプリ内へコピーして追加しました。"));
                }
            }
            Err(error) => {
                // import_managed_cube_file は書き込み失敗時に部分ファイルを除去する。
                // 予約 ID は transaction の cancel cleanup 対象から外してよい。
                self.creative_lut_transaction.forget_created(reserved_id);
                self.creative_lut_message = Some(error);
            }
        }
    }

    pub(super) fn remove_creative_lut(&mut self, index: usize) -> Option<String> {
        let removed = self.settings.creative_luts.get(index)?;
        if removed.is_builtin() {
            return None;
        }
        let removed = self.settings.creative_luts.remove(index);
        self.creative_lut_transaction.mark_removed(removed.id);
        if self.settings.video_adjustments.creative_lut.id == Some(removed.id) {
            self.settings.video_adjustments.creative_lut.id = None;
        }
        Some(removed.name)
    }
}

fn path_status(result: Option<Result<bool, String>>) -> ExternalToolPathStatus {
    match result {
        None => ExternalToolPathStatus::Unchecked,
        Some(Ok(true)) => ExternalToolPathStatus::Valid,
        Some(Ok(false)) => ExternalToolPathStatus::Invalid,
        Some(Err(error)) => ExternalToolPathStatus::Error(error),
    }
}

fn ui_font_settings_key(settings: &crate::settings::UiFontSettings) -> String {
    serde_json::to_string(settings).unwrap_or_else(|_| format!("{settings:?}"))
}

fn advance_preferences_scroll_generation(
    previous: PreferencesPage,
    current: PreferencesPage,
    generation: &mut u64,
) {
    if previous != current {
        *generation = generation.wrapping_add(1);
    }
}

fn advance_preferences_scroll_generation_on_open(
    show_preferences: bool,
    open_last_frame: &mut bool,
    sequence: &mut u64,
) -> Option<u64> {
    let opened = show_preferences && !*open_last_frame;
    *open_last_frame = show_preferences;
    opened.then(|| {
        *sequence = sequence.wrapping_add(1);
        *sequence
    })
}

const PREFERENCE_HIGHLIGHT_SECS: f64 = 2.5;

/// ページ内の検索対象コントロールへ anchor を付ける。
/// ui.scope の既存レイアウトをそのまま使い、余白や折り返し幅は追加しない。
pub(super) fn anchored<R>(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    anchor: &'static str,
    add: impl FnOnce(&mut egui::Ui, &mut PreferencesState) -> R,
) -> R {
    let response = ui.scope(|ui| add(ui, state));
    let rect = response.response.rect;

    if state.pending_anchor == Some(anchor) {
        ui.scroll_to_rect(rect, Some(egui::Align::Center));
        state.pending_anchor = None;
        state.highlight = Some((anchor, ui.ctx().input(|input| input.time)));
    }

    if let Some((highlighted, started_at)) = state.highlight
        && highlighted == anchor
    {
        let now = ui.ctx().input(|input| input.time);
        if now - started_at <= PREFERENCE_HIGHLIGHT_SECS {
            ui.painter().rect_stroke(
                rect.expand(2.0),
                4.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Outside,
            );
            ui.ctx().request_repaint();
        } else {
            state.highlight = None;
        }
    }

    response.inner
}

fn preference_category(page: PreferencesPage) -> (&'static str, usize, usize) {
    for (category_index, category) in TREE.iter().enumerate() {
        if category.page == Some(page) {
            return (category.label, category_index, 0);
        }
        for (page_index, candidate) in category.children.iter().enumerate() {
            if *candidate == page {
                return (category.label, category_index, page_index);
            }
        }
    }
    unreachable!("every PreferencesPage must be present in TREE")
}

fn select_preference_search_result(state: &mut PreferencesState, entry: &PrefSearchEntry) {
    state.selected = entry.page;
    state.right_panel_scroll_generation = state.right_panel_scroll_generation.wrapping_add(1);
    state.pending_anchor = Some(entry.anchor);
    state.highlight = None;
    state.showing_results = false;
    let (category, _, _) = preference_category(entry.page);
    if TREE
        .iter()
        .find(|candidate| candidate.label == category)
        .is_some_and(|candidate| !candidate.children.is_empty())
    {
        state.expanded.insert(category);
    }
}

fn draw_preference_search(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let width = ui.available_width();
    let response = crate::ime_focus::add_singleline(ui, &mut state.search_query, None, |edit| {
        edit.desired_width(width).hint_text("設定を検索")
    });
    if response.changed() {
        state.showing_results = !state.search_query.trim().is_empty();
    }
    ui.add_space(6.0);
}

fn draw_preference_search_results(
    ui: &mut egui::Ui,
    query: &str,
) -> Option<&'static PrefSearchEntry> {
    ui.heading("検索結果");
    ui.add_space(8.0);
    let results = search_preferences(query, preference_category);
    if results.is_empty() {
        ui.label("一致する設定がありません");
        return None;
    }
    let mut selected = None;
    for entry in results {
        let category = preference_category(entry.page).0;
        let mut category_path = category.to_owned();
        category_path.push(' ');
        category_path.push('\u{203a}');
        category_path.push(' ');
        category_path.push_str(entry.page.label());
        let row_height = ui.spacing().interact_size.y + 8.0;
        let size = egui::vec2(ui.available_width(), row_height);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.bg_fill);
        }
        let left_max_x = (rect.max.x - 155.0).max(rect.min.x);
        let left_rect = egui::Rect::from_min_max(rect.min, egui::pos2(left_max_x, rect.max.y));
        ui.painter().with_clip_rect(left_rect).text(
            egui::pos2(rect.min.x + 6.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            entry.title,
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().text_color(),
        );
        ui.painter().text(
            egui::pos2(rect.max.x - 6.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            category_path,
            egui::TextStyle::Small.resolve(ui.style()),
            ui.visuals().weak_text_color(),
        );
        if response.clicked() {
            selected = Some(entry);
        }
    }
    selected
}

/// 指定ディレクトリ配下のファイル合計サイズを返す。エラーや不在は 0。
/// AI バックエンドページの "X MiB を解放" 表示等で使う。
fn dir_size_bytes(dir: &std::path::Path) -> u64 {
    fn walk(p: &std::path::Path) -> u64 {
        let Ok(meta) = std::fs::metadata(p) else {
            return 0;
        };
        if meta.is_file() {
            return meta.len();
        }
        let mut total = 0u64;
        if let Ok(entries) = std::fs::read_dir(p) {
            for e in entries.flatten() {
                total = total.saturating_add(walk(&e.path()));
            }
        }
        total
    }
    walk(dir)
}

// ── メインダイアログ ────────────────────────────────────────────

fn prepare_preferences_settings_for_commit(
    edited: &mut crate::settings::Settings,
    live: &mut crate::settings::Settings,
) {
    let old_details_selection_bar_mode = live.details_selection_bar_mode.normalized();
    let new_details_selection_bar_mode = edited.details_selection_bar_mode.normalized();

    edited.overwrite_non_preferences_from(live);

    // セット C は上の移送対象なので、スナップショット上で先に複製すると live 値で
    // 上書きされて消える。旧 live → 新 snapshot が Dedicated へ入る遷移だけ、移送後に複製する。
    edited.details_selection_bar_mode = new_details_selection_bar_mode;
    if old_details_selection_bar_mode != crate::settings::DetailsSelectionBarMode::Dedicated
        && new_details_selection_bar_mode == crate::settings::DetailsSelectionBarMode::Dedicated
    {
        edited.copy_details_columns_to_selection_bar();
    }

    edited.reading_history_limit = edited
        .reading_history_limit
        .clamp(1, crate::reading_history_db::READING_HISTORY_LIMIT_MAX);
}

impl App {
    pub(crate) fn open_preferences_page(&mut self, page: PreferencesPage) {
        self.preferences_requested_page = Some(page);
        self.show_preferences = true;
    }

    fn preferences_dialog_has_unsaved_changes(&self) -> bool {
        let Some(state) = self.pref_state.as_ref() else {
            return false;
        };
        let mut edited = state.settings.clone();
        let mut live = self.settings.clone();
        edited.overwrite_non_preferences_from(&mut live);
        !settings_equal_for_close_prompt(&edited, &self.settings)
    }

    fn operation_customize_dialog_has_unsaved_changes(&self) -> bool {
        let Some(state) = self.operation_customize_state.as_ref() else {
            return false;
        };
        let mut edited_ring = state.settings.ring_shortcuts.clone();
        edited_ring.sanitize();
        state.settings.keymap != self.settings.keymap || edited_ring != self.settings.ring_shortcuts
    }

    fn request_close_preferences_dialog(&mut self) {
        if self.preferences_dialog_has_unsaved_changes() {
            self.show_preferences = true;
            self.show_preferences_discard_confirm = true;
        } else {
            self.discard_preferences_dialog();
        }
    }

    fn discard_preferences_dialog(&mut self) {
        self.pref_state = None;
        self.show_preferences = false;
        self.show_preferences_discard_confirm = false;
    }

    fn request_close_operation_customize_dialog(&mut self) {
        if self.operation_customize_dialog_has_unsaved_changes() {
            self.show_operation_customize = true;
            self.show_operation_customize_discard_confirm = true;
        } else {
            self.discard_operation_customize_dialog();
        }
    }

    fn discard_operation_customize_dialog(&mut self) {
        self.operation_customize_state = None;
        self.show_operation_customize = false;
        self.show_operation_customize_discard_confirm = false;
    }

    fn draw_preferences_discard_confirm(&mut self, ctx: &egui::Context) {
        if !self.show_preferences_discard_confirm {
            return;
        }
        match draw_discard_changes_confirm(
            ctx,
            "preferences_discard_changes_confirm",
            "環境設定の変更を破棄しますか？",
            "OK で適用していない変更があります。",
        ) {
            Some(true) => self.discard_preferences_dialog(),
            Some(false) => self.show_preferences_discard_confirm = false,
            None => {}
        }
    }

    fn draw_operation_customize_discard_confirm(&mut self, ctx: &egui::Context) {
        if !self.show_operation_customize_discard_confirm {
            return;
        }
        match draw_discard_changes_confirm(
            ctx,
            "operation_customize_discard_changes_confirm",
            "操作カスタマイズの変更を破棄しますか？",
            "OK で適用していない割り当て変更があります。",
        ) {
            Some(true) => self.discard_operation_customize_dialog(),
            Some(false) => self.show_operation_customize_discard_confirm = false,
            None => {}
        }
    }

    pub(crate) fn show_preferences_dialog(&mut self, ctx: &egui::Context) {
        // ScrollArea の id は open edge でだけ変える。毎フレーム変えると利用者が
        // スクロールできない。sequence は PreferencesState より長寿命にして、閉じて
        // state が破棄されても次回 open で過去の id を再利用しない。
        let opened_scroll_generation = advance_preferences_scroll_generation_on_open(
            self.show_preferences,
            &mut self.preferences_open_last_frame,
            &mut self.preferences_right_panel_scroll_sequence,
        );
        if !self.show_preferences {
            return;
        }

        // 初回: 一時コピーを作成
        if self.pref_state.is_none() {
            let external_tool_target = crate::external_tool::LaunchTarget::from_grid_item(
                self.fullscreen_idx
                    .or(self.selected)
                    .and_then(|index| self.items.get(index)),
            );
            #[cfg_attr(not(windows), allow(unused_mut))]
            let mut new_state = PreferencesState::from_settings(
                &self.settings,
                external_tool_target,
                self.ai_runtime.as_deref(),
                self.audio_normalize_db.is_some(),
                self.audio_normalize_db
                    .as_ref()
                    .map(|db| db.count())
                    .unwrap_or(0),
                self.book_resume_db
                    .as_ref()
                    .map(|db| db.count())
                    .unwrap_or(0),
                self.reading_history_db
                    .as_ref()
                    .map(|db| db.count())
                    .unwrap_or(0),
            );
            // 既にスキャン済みの VST3 プラグイン候補を引き継ぐ (= 再スキャン不要で表示)
            #[cfg(windows)]
            {
                new_state.vst3_discovered = self.vst3_discovered.clone();
            }
            self.pref_state = Some(new_state);
        }
        if let Some(generation) = opened_scroll_generation {
            self.pref_state
                .as_mut()
                .expect("preferences state was initialized above")
                .right_panel_scroll_generation = generation;
        }

        if let Some(requested) = self.preferences_requested_page.take() {
            if let Some(state) = self.pref_state.as_mut() {
                let previous = state.selected;
                state.selected = requested;
                advance_preferences_scroll_generation(
                    previous,
                    state.selected,
                    &mut state.right_panel_scroll_generation,
                );
            }
        }

        // 毎フレーム refresh: auto-bypass されたスロットを取得して PreferencesState に反映。
        // VST3 ページ下部の赤字警告表示用。最新状態を見せたいので毎フレーム更新する
        // (= ユーザーが手動で再 ON した瞬間に警告が消える等、即時反映)。
        #[cfg(windows)]
        if let Some(state) = self.pref_state.as_mut() {
            let sample_rate = self.dsp_bridge.sample_rate();
            state.vst3_auto_bypassed = self
                .dsp_bridge
                .slots()
                .into_iter()
                .filter(|s| s.auto_bypassed_for_latency && s.bypass)
                .map(|s| {
                    let ms = if sample_rate > 0 {
                        s.latency_samples as f64 / sample_rate as f64 * 1000.0
                    } else {
                        0.0
                    };
                    (s.plugin_name.unwrap_or_else(|| "(不明)".to_string()), ms)
                })
                .collect();
        }

        let mut open = true;
        let mut apply = false;
        let mut cancel = false;

        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);

        egui::Window::new("環境設定")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos(dialog_pos)
            .default_size([720.0, 520.0])
            .show(ctx, |ui| {
                let state = self.pref_state.as_mut().unwrap();
                state.poll_external_tool_workers(ctx);
                if state.ui_font_catalog_started {
                    state.poll_ui_font_tasks(ctx);
                }

                // ── メインエリア: 左ツリー + 右パネル ──
                let available = ui.available_size();
                let bottom_height = 36.0;
                let main_height = (available.y - bottom_height - 12.0).max(200.0);
                let tree_width = 180.0;

                // StripBuilder の代わりに手動で左右分割
                // 左ツリーを child_ui で配置し、残りを右パネルにする
                let outer_rect = ui.available_rect_before_wrap();
                let left_rect =
                    egui::Rect::from_min_size(outer_rect.min, egui::vec2(tree_width, main_height));
                let right_rect = egui::Rect::from_min_size(
                    egui::pos2(outer_rect.min.x + tree_width + 8.0, outer_rect.min.y),
                    egui::vec2(
                        (outer_rect.width() - tree_width - 8.0).max(100.0),
                        main_height,
                    ),
                );

                // 左ツリー
                let mut left_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(left_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                let selected_before_tree = state.selected;
                draw_preference_search(&mut left_ui, state);
                let tree_height = left_ui.available_height().max(0.0);
                egui::ScrollArea::vertical()
                    .id_salt("pref_tree")
                    .max_height(tree_height)
                    .auto_shrink([false, false])
                    .show(&mut left_ui, |ui| {
                        ui.set_min_width(tree_width - 12.0);
                        draw_tree(ui, state);
                    });
                advance_preferences_scroll_generation(
                    selected_before_tree,
                    state.selected,
                    &mut state.right_panel_scroll_generation,
                );

                // 区切り線
                let sep_x = outer_rect.min.x + tree_width + 3.0;
                ui.painter().vline(
                    sep_x,
                    outer_rect.min.y..=outer_rect.min.y + main_height,
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );

                let mut right_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(right_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                // 右パネルは solid スクロールバーで実幅を確保する。既定の floating
                // スクロールバーは本文上に重なるため、右端ボタンや区切り線が読みにくくなる。
                right_ui.spacing_mut().scroll = pref_panel_scroll_style();
                let mut command_capture_waiting = false;
                if enter_pressed && state.showing_results {
                    let first = search_preferences(&state.search_query, preference_category)
                        .into_iter()
                        .next();
                    if let Some(entry) = first {
                        select_preference_search_result(state, entry);
                    }
                }
                egui::ScrollArea::vertical()
                    .id_salt(("pref_panel", state.right_panel_scroll_generation))
                    .scroll_bar_visibility(
                        egui::containers::scroll_area::ScrollBarVisibility::AlwaysVisible,
                    )
                    .auto_shrink([false, false])
                    .max_height(main_height)
                    .show(&mut right_ui, |ui| {
                        ui.set_width(ui.available_width());
                        command_capture_waiting = state.command_capture_slot.is_some();
                        if state.showing_results {
                            let query = state.search_query.clone();
                            if let Some(entry) = draw_preference_search_results(ui, &query) {
                                select_preference_search_result(state, entry);
                            }
                        } else {
                            draw_page(ui, state, enter_pressed);
                        }
                    });

                // 全体の高さを確保
                ui.allocate_space(egui::vec2(available.x, main_height));

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // Esc でキャンセル (IME 変換中はスキップ)
                if escape_pressed && !command_capture_waiting {
                    if state.showing_results || !state.search_query.is_empty() {
                        state.search_query.clear();
                        state.showing_results = false;
                    } else {
                        cancel = true;
                    }
                }

                ui.horizontal(|ui| {
                    let font_ready = state.ui_font_apply_ready();
                    let lut_ready = state.creative_lut_import_rx.is_none();
                    if ui
                        .add_enabled(font_ready && lut_ready, egui::Button::new("  OK  "))
                        .clicked()
                    {
                        apply = true;
                        // (note: 「TRT 全エンジンビルド」ボタンのフラグは下のブロックで処理する)
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                    if !font_ready {
                        ui.small("フォントの準備完了後に適用できます。");
                    } else if !lut_ready {
                        ui.small("LUTのコピー完了後に適用できます。");
                    }
                });
            });

        if let Some(state) = self.pref_state.as_ref() {
            self.preferences_right_panel_scroll_sequence = state.right_panel_scroll_generation;
        }
        let external_launch = self
            .pref_state
            .as_mut()
            .and_then(|state| state.external_tool_launch_requested.take());
        if let Some((tool, target)) = external_launch {
            self.queue_external_tool_launch(&tool, &target);
        }

        let mut close_requested_this_frame = false;
        if apply {
            if let Some(mut state) = self.pref_state.take() {
                let old_dup = (
                    self.settings.skip_zip_if_folder_exists,
                    self.settings.skip_archive_if_zip_exists,
                    self.settings.skip_image_if_video_exists,
                    self.settings.skip_duplicate_images,
                    self.settings.image_ext_priority.clone(),
                    self.settings.video_thumb_use_sidecar_image,
                    self.settings.archive_file_handling_resolved(),
                );
                let old_grid_display_order = self.settings.grid_display_order.clone();
                let old_show_hidden_files = self.settings.show_hidden_files;
                let old_exif = self.settings.exif_hidden_tags.clone();

                let old_susie = (
                    self.settings.susie_enabled,
                    self.settings.susie_allow_parallel,
                );

                let old_pause_minimized = self.settings.pause_indexer_while_minimized;

                // 動画ループモード変更を検出してフルスクリーン中の player に反映する
                let old_loop_mode = self.settings.video_loop_mode;

                // AI バックエンド設定変更を検出してホットリロードトリガに使う
                let old_ai_backend = self.settings.ai_backend.clone();
                let new_ai_backend = state.settings.ai_backend.clone();
                let old_ai_feature_mode = self.settings.ai_feature_mode;
                let old_reading_history_limit = self.settings.reading_history_limit;
                let old_keymap_settings = self.settings.keymap.clone();
                let old_fullscreen_side_panel_mode =
                    self.settings.fullscreen_side_panel_mode.normalized();
                let old_ui_font = self.settings.ui_font.clone();
                let old_creative_luts = self.settings.creative_luts.clone();
                let mut creative_lut_transaction =
                    std::mem::take(&mut state.creative_lut_transaction);

                // AI 処理サイズ上限の変更検出 (final AI cache / failed / pending の
                // 無効化トリガに使う)
                let old_ai_size_limits = (
                    self.settings.ai_upscale_limit(),
                    self.settings.ai_denoise_limit(),
                );
                let old_retained_final_ai_cache_budget = (
                    self.settings.retained_final_ai_cache_max_entries,
                    self.settings.retained_final_ai_cache_max_mib,
                );
                let old_edit_preview_cache = (
                    self.settings.edit_preview_cache_enabled,
                    self.settings.edit_preview_cache_max_bytes,
                );
                let old_edit_restore_prompt_enabled = self.settings.edit_restore_prompt_enabled;
                #[cfg(windows)]
                let old_video_seek_strip_min_interval_secs =
                    self.settings.video_seek_strip_min_interval_secs;
                #[cfg(windows)]
                let old_video_seek_strip_waveform_span_secs =
                    self.settings.video_seek_strip_waveform_span_secs;

                // VST3 enable 状態 + チェーン構成の変化を検出してホットリロード。
                let old_vst3_enabled = self.settings.vst3_enabled;
                let new_vst3_enabled = state.settings.vst3_enabled;
                #[cfg(windows)]
                let old_vst3_chain: Vec<String> = self
                    .settings
                    .vst3_plugins
                    .iter()
                    .map(|e| e.path.clone())
                    .collect();
                #[cfg(windows)]
                let new_vst3_chain: Vec<String> = state
                    .settings
                    .vst3_plugins
                    .iter()
                    .map(|e| e.path.clone())
                    .collect();
                #[cfg(windows)]
                let old_detached_open_images_in_window =
                    self.settings.detached_viewer_open_images_in_window;
                #[cfg(windows)]
                let old_fullfeature_media_window = self.settings.fullfeature_media_window;
                // VST3 ページで再スキャンした候補を App 側に反映
                #[cfg(windows)]
                let new_vst3_discovered = state.vst3_discovered.clone();

                // ダイアログを開いた時点の `state.settings` は self.settings の snapshot。
                // 開いている間に他ダイアログ (お気に入り編集 / タグ編集 / 補正プリセット /
                // 開いたアプリ履歴等) や runtime (ツールバー選択 / ウィンドウ移動 / レーティング
                // フィルタ) が変えたフィールドは state.settings 側に反映されないため、そのまま
                // self.settings = state.settings を実行するとそれらの変更が消える。
                // 対策: 環境設定が管理しないフィールドを self.settings の最新値で state に
                // 移送してから全体差し替えする。新しく「環境設定 UI から触らない」フィールドを
                // Settings に追加した場合はここにも追記が必要。
                prepare_preferences_settings_for_commit(&mut state.settings, &mut self.settings);
                self.settings = state.settings;
                #[cfg(windows)]
                if old_video_seek_strip_min_interval_secs.to_bits()
                    != self.settings.video_seek_strip_min_interval_secs.to_bits()
                {
                    self.rebuild_video_seek_strip_adopted_list();
                }
                #[cfg(windows)]
                if old_video_seek_strip_waveform_span_secs.to_bits()
                    != self.settings.video_seek_strip_waveform_span_secs.to_bits()
                {
                    self.rebuild_video_seek_strip_waveform_span();
                }
                self.sync_content_identity_detection_setting(old_edit_restore_prompt_enabled);
                if old_creative_luts != self.settings.creative_luts {
                    if self
                        .settings
                        .video_adjustments
                        .creative_lut
                        .id
                        .is_some_and(|id| {
                            !self
                                .settings
                                .creative_luts
                                .iter()
                                .any(|entry| entry.id == id)
                        })
                    {
                        self.settings.video_adjustments.creative_lut.id = None;
                    }
                    self.creative_lut_library
                        .reload(&self.settings.creative_luts);
                    self.clear_all_final_pipeline_caches();
                    #[cfg(windows)]
                    self.sync_native_video_grade();
                }
                if old_ui_font != self.settings.ui_font {
                    #[cfg(windows)]
                    {
                        // detached viewer / native HUD はそれぞれ独立した egui Context を
                        // 持つため、表示倍率変更と同じ正規 teardown 経路で閉じる。
                        // 再度開いた時点で新しいフォント設定を使って生成される。
                        let closed_detached = self.close_all_detached_viewers_for_mode_change(ctx);
                        if !closed_detached && self.fullscreen_idx.is_some() {
                            self.close_fullscreen();
                        }
                        self.request_main_font_update();
                    }
                    #[cfg(not(windows))]
                    crate::ui_fonts::configure_fonts_with_settings(ctx, &self.settings.ui_font);
                }
                if old_fullscreen_side_panel_mode
                    != self.settings.fullscreen_side_panel_mode.normalized()
                {
                    self.reset_fs_side_panel_runtime_for_mode_change();
                }
                if old_keymap_settings != self.settings.keymap {
                    let keymap = crate::keymap::Keymap::from_settings(&self.settings.keymap);
                    for warning in keymap.warnings() {
                        crate::logger::log(format!("[keymap] {warning}"));
                    }
                    keymap.install_global_native_video_shortcuts();
                    self.keymap = keymap;
                }
                #[cfg(windows)]
                if (old_detached_open_images_in_window
                    != self.settings.detached_viewer_open_images_in_window
                    || old_fullfeature_media_window != self.settings.fullfeature_media_window)
                    && self.close_all_detached_viewers_for_mode_change(ctx)
                {
                    self.show_feedback_toast(
                        "別ウィンドウの表示モードを変更したため、開いていた別ウィンドウを閉じました"
                            .to_string(),
                    );
                }
                // (フォルダ履歴 / A・B 記憶のクリアは v2.0.0 でフォルダバーの右クリック
                //  メニューへ移動。環境設定 OK 経路でのクリア要求は廃止。)
                if old_reading_history_limit != self.settings.reading_history_limit {
                    if let Some(writer) = &self.reading_history_writer {
                        writer.prune(self.settings.reading_history_limit);
                    } else if let Some(db) = self.reading_history_db.as_ref() {
                        if let Err(e) = db.prune(self.settings.reading_history_limit) {
                            crate::logger::log(format!("reading-history prune failed: {e}"));
                        }
                    }
                    if self.items_are_reading_history_view {
                        self.enter_reading_history();
                    }
                }
                self.settings.save();
                creative_lut_transaction.commit();

                if let Some(service) = &self.edit_preview_cache {
                    if !self.settings.edit_preview_cache_enabled {
                        if old_edit_preview_cache.0 {
                            service.clear();
                        }
                    } else if old_edit_preview_cache
                        != (
                            self.settings.edit_preview_cache_enabled,
                            self.settings.edit_preview_cache_max_bytes,
                        )
                    {
                        service.prune(self.settings.edit_preview_cache_max_bytes.max(1_000_000));
                    }
                }

                // 動画ループモードが変わったらフルスクリーン中の player に反映する
                #[cfg(windows)]
                if old_loop_mode != self.settings.video_loop_mode {
                    if let Some(idx) = self.fullscreen_idx {
                        self.apply_loop_mode_to_player(idx);
                    }
                }
                #[cfg(not(windows))]
                let _ = old_loop_mode;

                // 「常駐中はインデックス更新を一時停止する」が変わったらトレイの checkmark も
                // 同期する (お気に入り編集ダイアログと同じチェックボックス項目への二重経路)。
                if old_pause_minimized != self.settings.pause_indexer_while_minimized {
                    self.sync_tray_pause_check();
                }

                // AI バックエンド変更のホットリロード処理 (Phase 3)
                if old_ai_backend != new_ai_backend {
                    self.apply_ai_backend_change(new_ai_backend.as_deref());
                }
                if old_ai_feature_mode != self.settings.ai_feature_mode {
                    self.apply_ai_feature_mode_change();
                }
                let new_ai_size_limits = (
                    self.settings.ai_upscale_limit(),
                    self.settings.ai_denoise_limit(),
                );
                if old_ai_size_limits != new_ai_size_limits {
                    self.apply_ai_size_limit_change();
                }
                let new_retained_final_ai_cache_budget = (
                    self.settings.retained_final_ai_cache_max_entries,
                    self.settings.retained_final_ai_cache_max_mib,
                );
                if old_retained_final_ai_cache_budget != new_retained_final_ai_cache_budget {
                    self.prune_retained_final_ai_cache_to_settings();
                }

                // 同名ファイル設定が変更された場合はフォルダを再読み込み
                // (Phase 5.3: video_thumb_use_sidecar_image も含む — 切り替え時に
                // video_thumb_overrides を作り直すため)
                let new_dup = (
                    self.settings.skip_zip_if_folder_exists,
                    self.settings.skip_archive_if_zip_exists,
                    self.settings.skip_image_if_video_exists,
                    self.settings.skip_duplicate_images,
                    self.settings.image_ext_priority.clone(),
                    self.settings.video_thumb_use_sidecar_image,
                    self.settings.archive_file_handling_resolved(),
                );
                let duplicate_settings_changed = old_dup != new_dup;
                let file_visibility_changed =
                    old_show_hidden_files != self.settings.show_hidden_files;
                let grid_display_order_changed =
                    old_grid_display_order != self.settings.grid_display_order;
                if duplicate_settings_changed || file_visibility_changed {
                    self.reload_current_folder_preserving_override();
                } else if grid_display_order_changed {
                    self.apply_sort_change_reload();
                }
                if old_exif != self.settings.exif_hidden_tags {
                    self.exif_cache.clear();
                }

                // VST3 enable 状態 / チェーン構成の変化反映
                #[cfg(windows)]
                {
                    self.vst3_discovered = new_vst3_discovered;
                    let chain_changed = old_vst3_chain != new_vst3_chain;
                    if old_vst3_enabled != new_vst3_enabled {
                        if new_vst3_enabled {
                            self.kick_off_vst3_chain_rebuild();
                        } else {
                            // VST3 OFF へのトグル: bridge teardown 前に内部状態と
                            // ウィンドウ位置を保存 (= 次回 ON 時の復元用)。
                            // T22: UI スレッドなので 2 秒に制限
                            let states = self.snapshot_vst3_states_into_settings(
                                std::time::Duration::from_secs(2),
                            );
                            let positions = self.snapshot_vst3_window_positions_into_settings();
                            if states > 0 || positions > 0 {
                                self.settings.save();
                            }
                            self.dsp_bridge.disable();
                        }
                    } else if chain_changed && new_vst3_enabled {
                        self.kick_off_vst3_chain_rebuild();
                    }
                }
                #[cfg(not(windows))]
                let _ = (old_vst3_enabled, new_vst3_enabled);

                let new_susie = (
                    self.settings.susie_enabled,
                    self.settings.susie_allow_parallel,
                );
                if old_susie != new_susie {
                    crate::susie_loader::reload(
                        self.settings.susie_enabled,
                        self.settings.susie_allow_parallel,
                    );
                    // 対応拡張子が変わる可能性があるので現在のフォルダを再読み込み
                    self.reload_current_folder_preserving_override();
                }
            }
            self.show_preferences = false;
            self.show_preferences_discard_confirm = false;
        } else if cancel || !open {
            close_requested_this_frame = true;
            self.request_close_preferences_dialog();
        }

        if !close_requested_this_frame {
            self.draw_preferences_discard_confirm(ctx);
        }

        let mut clear_audio_normalize_requested = false;
        if let Some(ps) = self.pref_state.as_mut() {
            if ps.audio_normalize_clear_requested {
                ps.audio_normalize_clear_requested = false;
                clear_audio_normalize_requested = true;
            }
        }
        if clear_audio_normalize_requested {
            let result = match self.audio_normalize_db.as_ref() {
                Some(db) => match db.clear_all() {
                    Ok(deleted) => {
                        let remaining = db.count();
                        crate::logger::log(format!(
                            "[audio_normalize] cleared {deleted} cached measurements"
                        ));
                        Ok((deleted, remaining))
                    }
                    Err(e) => Err(format!("{e}")),
                },
                None => Err("音量ノーマライズ測定値 DB を開けませんでした".to_string()),
            };

            if let Some(ps) = self.pref_state.as_mut() {
                match result {
                    Ok((deleted, remaining)) => {
                        ps.audio_normalize_entry_count = remaining;
                        ps.audio_normalize_clear_result = Some(format!(
                            "音量ノーマライズ測定値を {deleted} 件削除しました。"
                        ));
                    }
                    Err(err) => {
                        ps.audio_normalize_clear_result =
                            Some(format!("音量ノーマライズ測定値の削除に失敗しました: {err}"));
                    }
                }
            }
        }

        // 履歴と復元ページ: ZIP/PDF 読書位置クリア (one-shot)。
        let mut clear_book_resume_requested = false;
        if let Some(ps) = self.pref_state.as_mut() {
            if ps.book_resume_clear_requested {
                ps.book_resume_clear_requested = false;
                clear_book_resume_requested = true;
            }
        }
        if clear_book_resume_requested {
            let result = match self.book_resume_db.as_ref() {
                Some(db) => match db.clear_all() {
                    Ok(deleted) => {
                        crate::logger::log(format!(
                            "[book_resume] cleared {deleted} reading positions"
                        ));
                        Ok((deleted, db.count()))
                    }
                    Err(e) => Err(format!("{e}")),
                },
                None => Err("読書位置 DB を開けませんでした".to_string()),
            };
            if let Some(ps) = self.pref_state.as_mut() {
                match result {
                    Ok((deleted, remaining)) => {
                        ps.book_resume_entry_count = remaining;
                        ps.book_resume_clear_result =
                            Some(format!("ZIP/PDF の読書位置を {deleted} 件削除しました。"));
                    }
                    Err(err) => {
                        ps.book_resume_clear_result =
                            Some(format!("読書位置の削除に失敗しました: {err}"));
                    }
                }
            }
        }

        // 履歴と復元ページ: 閲覧履歴クリア (one-shot)。
        let mut clear_reading_history_requested = false;
        if let Some(ps) = self.pref_state.as_mut() {
            if ps.reading_history_clear_requested {
                ps.reading_history_clear_requested = false;
                clear_reading_history_requested = true;
            }
        }
        if clear_reading_history_requested {
            let result = match self.reading_history_db.as_ref() {
                Some(db) => match db.clear_all() {
                    Ok(deleted) => {
                        crate::logger::log(format!("[reading_history] cleared {deleted} entries"));
                        Ok((deleted, db.count()))
                    }
                    Err(e) => Err(format!("{e}")),
                },
                None => Err("閲覧履歴 DB を開けませんでした".to_string()),
            };
            if let Some(ps) = self.pref_state.as_mut() {
                match result {
                    Ok((deleted, remaining)) => {
                        ps.reading_history_entry_count = remaining;
                        ps.reading_history_clear_result =
                            Some(format!("閲覧履歴を {deleted} 件削除しました。"));
                    }
                    Err(err) => {
                        ps.reading_history_clear_result =
                            Some(format!("閲覧履歴の削除に失敗しました: {err}"));
                    }
                }
            }
            if self.items_are_reading_history_view {
                self.enter_reading_history();
            }
        }

        // 「TensorRT パックをダウンロード」ボタンが押されていたら、環境設定ダイアログを
        // 閉じて TRT install dialog を開く (= ユーザーは設定変更を保存しなくてもインストール
        // フローへ進める。完了後の TensorRT 有効化は再起動 + AiBackend 設定で行う想定)。
        if let Some(ps) = self.pref_state.as_mut() {
            if ps.start_trt_install_requested {
                ps.start_trt_install_requested = false;
                self.pref_state = None;
                self.show_preferences = false;
                let target_sm = crate::gpu_info::query_primary_gpu_sm();
                self.trt_install_state = Some(
                    crate::ui_dialogs::trt_install::TrtInstallState::new(target_sm),
                );
            }
        }

        // 「TensorRT パックを削除」ボタンが確定されていたら、worker pool を停止 →
        // ファイル削除 → live settings を DirectML に切替 → save をまとめて実行。
        if let Some(ps) = self.pref_state.as_mut() {
            if ps.uninstall_trt_pack_requested {
                ps.uninstall_trt_pack_requested = false;
                self.uninstall_trt_pack_now();
            }
        }

        // 「編集用追加ファイルをダウンロード / 更新」ボタンが押されていたら、環境設定を
        // 閉じて editing addon install dialog を開く (TRT と同じ即時実行フロー)。
        if let Some(ps) = self.pref_state.as_mut() {
            if ps.start_editing_addon_install_requested {
                ps.start_editing_addon_install_requested = false;
                self.pref_state = None;
                self.show_preferences = false;
                // 環境設定から明示的に要求したので、このセッションの辞退フラグは解除して
                // 確実に確認ダイアログを開く。
                self.editing_addon_declined_session = false;
                self.editing_addon_install_state =
                    Some(crate::ui_dialogs::editing_addon::EditingAddonInstallState::new());
            }
        }

        // 「編集用追加ファイルを削除」が確定されていたら背景削除する。
        if let Some(ps) = self.pref_state.as_mut() {
            if ps.uninstall_editing_addon_requested {
                ps.uninstall_editing_addon_requested = false;
                self.uninstall_editing_addon_now();
            }
        }

        // 「インストール先を開く」が押されていたら Explorer で開く。
        if let Some(ps) = self.pref_state.as_mut() {
            if ps.open_editing_addon_folder_requested {
                ps.open_editing_addon_folder_requested = false;
                self.open_editing_addon_folder();
            }
        }
    }

    pub(crate) fn show_operation_customize_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_operation_customize {
            return;
        }

        if self.operation_customize_state.is_none() {
            let mut state = PreferencesState::from_settings(
                &self.settings,
                crate::external_tool::LaunchTarget::None,
                self.ai_runtime.as_deref(),
                self.audio_normalize_db.is_some(),
                self.audio_normalize_db
                    .as_ref()
                    .map(|db| db.count())
                    .unwrap_or(0),
                self.book_resume_db
                    .as_ref()
                    .map(|db| db.count())
                    .unwrap_or(0),
                self.reading_history_db
                    .as_ref()
                    .map(|db| db.count())
                    .unwrap_or(0),
            );
            state.operation_tab = OperationCustomizeTab::Settings;
            self.operation_customize_state = Some(state);
        }

        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let ime_active = self.ime_input_active(ctx);
        let content_rect = ctx.content_rect();
        let safe_rect = content_rect.shrink2(egui::vec2(24.0, 32.0));
        let safe_size = safe_rect.size().max(egui::vec2(360.0, 300.0));
        let dialog_size = egui::vec2(900.0, 640.0).min(safe_size);
        let min_dialog_size = egui::vec2(560.0, 420.0).min(safe_size);
        let dialog_rect = egui::Rect::from_center_size(safe_rect.center(), dialog_size);

        egui::Window::new("操作カスタマイズ")
            .id(egui::Id::new("operation_customize_dialog_v2"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(dialog_rect.min)
            .default_size(dialog_size)
            .min_size(min_dialog_size)
            .max_size(safe_size)
            .constrain_to(safe_rect)
            .show(ctx, |ui| {
                let state = self.operation_customize_state.as_mut().unwrap();

                draw_operation_customize_tabs(ui, state);
                ui.separator();

                let bottom_height = 42.0;
                let available = ui.available_size();
                let main_height = (available.y - bottom_height - 12.0).max(140.0);
                let panel_size = egui::vec2(available.x, main_height);
                ui.allocate_ui_with_layout(
                    panel_size,
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_max_size(panel_size);
                        ui.spacing_mut().scroll = pref_panel_scroll_style();
                        egui::ScrollArea::vertical()
                            .id_salt("operation_customize_panel")
                            .scroll_bar_visibility(
                                egui::containers::scroll_area::ScrollBarVisibility::AlwaysVisible,
                            )
                            .auto_shrink([false, false])
                            .max_height(main_height)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                draw_operation_customize_page(ui, state, ime_active);
                            });
                    },
                );

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("  OK  ").clicked() {
                        apply = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                    if state.command_capture_slot.is_some() {
                        ui.small("キー入力待ち中です。Esc で入力待ちだけをキャンセルできます。");
                    }
                });
            });

        if let Some(state) = self.operation_customize_state.as_mut() {
            draw_operation_assignment_editor_dialog(ctx, state, ime_active);
            draw_mouse_gesture_recorder_dialog(ctx, state);
        }

        let mut close_requested_this_frame = false;
        if apply {
            if let Some(state) = self.operation_customize_state.take() {
                self.apply_operation_customize_state(state);
            }
            self.show_operation_customize = false;
            self.show_operation_customize_discard_confirm = false;
        } else if cancel || !open {
            close_requested_this_frame = true;
            self.request_close_operation_customize_dialog();
        }

        if !close_requested_this_frame {
            self.draw_operation_customize_discard_confirm(ctx);
        }
    }

    fn apply_operation_customize_state(&mut self, state: PreferencesState) {
        let mut bundle = crate::operation_customize_share::OperationCustomizeBundle::from_settings(
            &self.settings,
        );
        bundle.keymap = state.settings.keymap;
        bundle.ring_shortcuts = state.settings.ring_shortcuts;
        self.apply_operation_customize_bundle(bundle);
    }

    pub(crate) fn apply_operation_customize_bundle(
        &mut self,
        mut bundle: crate::operation_customize_share::OperationCustomizeBundle,
    ) {
        let old_keymap_settings = self.settings.keymap.clone();
        let old_ring_shortcuts = self.settings.ring_shortcuts.clone();
        bundle.ring_shortcuts.sanitize();
        bundle.apply_to(&mut self.settings);

        if old_keymap_settings != self.settings.keymap {
            let keymap = crate::keymap::Keymap::from_settings(&self.settings.keymap);
            for warning in keymap.warnings() {
                crate::logger::log(format!("[keymap] {warning}"));
            }
            keymap.install_global_native_video_shortcuts();
            self.keymap = keymap;
            #[cfg(windows)]
            {
                self.native_overlay_shortcut_help_cache = None;
            }
        }
        if old_ring_shortcuts != self.settings.ring_shortcuts {
            #[cfg(windows)]
            {
                self.set_native_video_ring_guide_overlay(None);
                self.set_native_video_ring_picker_overlay(None);
            }
        }
        self.settings.save();
    }

    /// 編集用追加パック削除フロー本体。
    /// - `active.json` を即削除 (= `addon_status()` が即 Missing になる、フォントは次回ベイクで外れる)
    /// - フォントキャッシュを無効化
    /// - 大きいモデルを含む `packs/` / `downloads/` の削除は背景 thread (UI を止めない、CLAUDE.md)
    pub(crate) fn uninstall_editing_addon_now(&mut self) {
        // 1. active pointer を先に外す (= 機能側は即「未導入」扱いになる)。
        let _ = std::fs::remove_file(crate::editing_addon::active_pointer_path());
        // 2. フォント / 被写体マットキャッシュ無効化。
        //    active pointer を外した後なので refresh は None になり、被写体マスク生成が即 disabled になる。
        self.comic_fonts = None;
        self.comic_fonts_loaded = false;
        self.comic_loaded_font_keys.clear();
        self.comic_font_registry_loaded = false; // pack フォントを一覧から外す
        // フォントソースが変わったので見本 / オノマトペプレビュー + 焼き済み注釈
        // (comic_cache) も無効化する (Codex P2)。
        self.font_sample_cache.clear();
        self.font_sample_failed.clear();
        self.onomatopoeia_thumb_cache.clear();
        self.mark_comic_dirty();
        // フォントソースは全ページに影響するので、比較 (Wipe/Diff) のピン留め側を含む準備済み
        // ピクセルも無効化する (mark_comic_dirty は現在ページしか落とさない、Codex P2)。
        self.invalidate_all_compare_prepared();
        self.refresh_subject_matte_path();
        crate::logger::log(
            "[editing pack] 削除を開始 (active.json 解除 + フォント/被写体マットキャッシュ無効化)"
                .to_string(),
        );
        // 3. pack ディレクトリ / DL 残骸の削除は背景 thread に逃がす
        //    (BiRefNet ~490MB を含むため remove_dir_all は数秒かかり得る)。
        let packs = crate::editing_addon::packs_root();
        let downloads = crate::editing_addon::downloads_dir();
        std::thread::Builder::new()
            .name("editing-pack-uninstall".to_string())
            .spawn(move || {
                for dir in [packs, downloads] {
                    match std::fs::remove_dir_all(&dir) {
                        Ok(()) => crate::logger::log(format!(
                            "[editing pack] 削除しました: {}",
                            dir.display()
                        )),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => crate::logger::log(format!(
                            "[editing pack] 削除に失敗: {} ({e})",
                            dir.display()
                        )),
                    }
                }
            })
            .ok();
    }

    /// 編集用追加パックのインストール先 (`%APPDATA%/mimageviewer/addons/editing/`) を
    /// Explorer で開く。未作成なら作ってから開く。
    pub(crate) fn open_editing_addon_folder(&mut self) {
        let dir = crate::editing_addon::addon_root();
        let _ = std::fs::create_dir_all(&dir);
        crate::ui_helpers::open_external_player(&dir);
    }

    /// TRT パック削除フロー本体。
    /// - 走行中の worker pool を detach (= 子プロセス停止 + DLL ハンドル解放、UI thread)
    /// - live `settings.ai_backend` を DirectML に切替して save (UI thread、瞬時)
    /// - `tensorrt/` と `tensorrt-engines/` を **背景 thread で** 削除 (= 多 GB の I/O で
    ///   UI thread をブロックしないため Codex P2 指摘)。削除完了は logger に出力するのみ
    ///   (UI 状態は呼び出し時点で既に「削除済」相当に同期されている)。
    pub(crate) fn uninstall_trt_pack_now(&mut self) {
        // 1. worker pool 停止 (DLL ハンドル解放)。これは速い (ms オーダー) ので UI thread で OK。
        if let Some(runtime) = self.ai_runtime.as_ref() {
            if runtime.has_worker_pool() {
                crate::logger::log(
                    "[AI] TRT パック削除のため worker pool を停止します".to_string(),
                );
                runtime.detach_worker_pool();
            }
        }

        // 2. live settings を DirectML に固定 → save (UI thread、瞬時)
        let was_trt =
            self.settings.ai_backend.as_deref() == Some(crate::ai::AiBackend::TensorRt.as_str());
        if was_trt {
            self.settings.ai_backend = Some(crate::ai::AiBackend::DirectMl.as_str().to_string());
            self.settings.save();
            crate::logger::log(
                "[AI] AI バックエンドを DirectML に切替しました (TRT パック削除後)".to_string(),
            );
        }

        // 3. ファイル削除は背景 thread に逃がす。
        //    pack ~2 GB + engine cache 数百 MB の remove_dir_all は秒〜十数秒かかり、
        //    UI を凍らせるため (CLAUDE.md: UI thread 同期 I/O 禁止)。
        //    削除完了前に再 install を要求した場合、install 側の atomic rename + INSTALL_OK
        //    最終書き込みパターンで上書きできるので race は実害なし。
        let pack_dir = crate::ai::tensorrt_pack::pack_dir();
        let engine_cache_dir = crate::ai::tensorrt_pack::engine_cache_dir();
        std::thread::Builder::new()
            .name("trt-pack-uninstall".to_string())
            .spawn(move || {
                for dir in [pack_dir, engine_cache_dir] {
                    match std::fs::remove_dir_all(&dir) {
                        Ok(()) => {
                            crate::logger::log(format!(
                                "[AI] TensorRT を削除しました: {}",
                                dir.display()
                            ));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            crate::logger::log(format!(
                                "[AI] TensorRT 削除に失敗: {} ({e})",
                                dir.display()
                            ));
                        }
                    }
                }
            })
            .ok();
    }
}

// ── ツリー描画 ──────────────────────────────────────────────────

fn draw_operation_customize_tabs(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.horizontal_wrapped(|ui| {
        for &tab in OperationCustomizeTab::all() {
            if ui
                .selectable_label(state.operation_tab == tab, tab.label())
                .clicked()
            {
                state.operation_tab = tab;
                state.command_capture_slot = None;
                state.command_edit_error = None;
                state.command_edit_notice = None;
            }
        }
    });
}

fn draw_operation_customize_page(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    ime_active: bool,
) {
    match state.operation_tab {
        OperationCustomizeTab::Settings => draw_operation_settings_page(ui, state),
        OperationCustomizeTab::Commands => {
            draw_operation_context_filter(ui, state);
            ui.add_space(8.0);
            page_command_overview(ui, state);
        }
        OperationCustomizeTab::RingShortcut => {
            draw_ring_context_tabs(ui, state);
            ui.add_space(8.0);
            let context = state.operation_ring_context;
            page_ring_shortcut_assignments(ui, state, context);
        }
        OperationCustomizeTab::Keyboard => {
            draw_operation_context_filter(ui, state);
            ui.add_space(8.0);
            ui.small("キー割り当てを編集します。一覧の「編集」またはキーボード図の割り当て済みキーを押すと、割り当て編集ダイアログを開きます。");
            ui.add_space(8.0);
            page_command_settings(ui, state, ime_active);
        }
        OperationCustomizeTab::MouseGesture => {
            draw_mouse_gesture_context_tabs(ui, state);
            ui.add_space(8.0);
            let context = state.operation_mouse_gesture_context;
            page_mouse_gesture_bindings(ui, state, context);
        }
        OperationCustomizeTab::Gamepad => {
            draw_gamepad_context_tabs(ui, state);
            ui.add_space(8.0);
            let context = state.operation_gamepad_context;
            page_gamepad_assignments(ui, state, context);
        }
    }
}

fn draw_operation_context_filter(ui: &mut egui::Ui, state: &mut PreferencesState) {
    use crate::keymap::KeyContext;

    const CONTEXTS: &[Option<KeyContext>] = &[
        None,
        Some(KeyContext::Global),
        Some(KeyContext::Grid),
        Some(KeyContext::FsCommon),
        Some(KeyContext::Rating),
        Some(KeyContext::FsImage),
        Some(KeyContext::FsVideo),
        Some(KeyContext::Erase),
    ];

    ui.horizontal(|ui| {
        ui.label("場所:");
        let selected = operation_keyboard_context_filter_label(state.operation_keyboard_context);
        egui::ComboBox::from_id_salt("operation_context_filter")
            .selected_text(selected)
            .width(190.0)
            .show_ui(ui, |ui| {
                for &context in CONTEXTS {
                    let label = operation_keyboard_context_filter_label(context);
                    if ui
                        .selectable_value(&mut state.operation_keyboard_context, context, label)
                        .clicked()
                    {
                        state.command_capture_slot = None;
                        state.command_edit_error = None;
                        state.command_edit_notice = None;
                    }
                }
            });
    });
}

fn draw_operation_settings_page(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.horizontal(|ui| {
        for tab in [
            OperationSettingsTab::Behavior,
            OperationSettingsTab::RightDrag,
            OperationSettingsTab::MouseButtons,
        ] {
            if ui
                .selectable_label(state.operation_settings_tab == tab, tab.label())
                .clicked()
            {
                state.operation_settings_tab = tab;
            }
        }
    });
    ui.add_space(8.0);
    match state.operation_settings_tab {
        OperationSettingsTab::Behavior => page_operation_behavior(ui, state),
        OperationSettingsTab::RightDrag => page_right_drag_modes(ui, state),
        OperationSettingsTab::MouseButtons => page_mouse_buttons(ui, state),
    }
}

fn draw_ring_context_tabs(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.horizontal_wrapped(|ui| {
        for &context in RingShortcutContext::all() {
            if ui
                .selectable_label(state.operation_ring_context == context, context.label())
                .clicked()
            {
                state.operation_ring_context = context;
            }
        }
    });
}

fn draw_gamepad_context_tabs(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.horizontal_wrapped(|ui| {
        for &context in RingShortcutContext::all() {
            if ui
                .selectable_label(state.operation_gamepad_context == context, context.label())
                .clicked()
            {
                state.operation_gamepad_context = context;
            }
        }
    });
}

fn draw_mouse_gesture_context_tabs(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.horizontal_wrapped(|ui| {
        for &context in RightDragContext::all() {
            if ui
                .selectable_label(
                    state.operation_mouse_gesture_context == context,
                    context.label(),
                )
                .clicked()
            {
                state.operation_mouse_gesture_context = context;
            }
        }
    });
}

fn draw_tree(ui: &mut egui::Ui, state: &mut PreferencesState) {
    for cat in TREE {
        if cat.children.is_empty() {
            // リーフカテゴリ（直接ページを持つ）
            if let Some(page) = cat.page {
                let selected = state.selected == page;
                if ui.selectable_label(selected, cat.label).clicked() {
                    state.selected = page;
                    state.showing_results = false;
                }
            }
        } else {
            // 子を持つカテゴリ
            let is_expanded = state.expanded.contains(cat.label);
            let icon = if is_expanded { "▼ " } else { "▶ " };
            let header_text = format!("{}{}", icon, cat.label);

            // カテゴリヘッダ: クリックで展開/折り畳み
            // カテゴリ自体がページを持つ場合は選択もする
            let is_cat_selected = cat.page.is_some_and(|p| state.selected == p);
            let resp =
                ui.selectable_label(is_cat_selected, egui::RichText::new(header_text).strong());
            if resp.clicked() {
                if is_expanded {
                    state.expanded.remove(cat.label);
                } else {
                    state.expanded.insert(cat.label);
                }
                if let Some(page) = cat.page {
                    state.selected = page;
                    state.showing_results = false;
                } else if !is_expanded
                    && !cat.children.contains(&state.selected)
                    && let Some(&first_child) = cat.children.first()
                {
                    state.selected = first_child;
                    state.showing_results = false;
                }
            }

            // 子ページ
            if is_expanded {
                for &child in cat.children {
                    let selected = state.selected == child;
                    let text = format!("    {}", child.label());
                    if ui.selectable_label(selected, text).clicked() {
                        state.selected = child;
                        state.showing_results = false;
                    }
                }
            }
        }
    }
}

// ── 右パネル ページ描画 ─────────────────────────────────────────

fn draw_page(ui: &mut egui::Ui, state: &mut PreferencesState, enter_pressed: bool) {
    ui.heading(state.selected.label());
    ui.add_space(8.0);

    match state.selected {
        PreferencesPage::General => page_general(ui, state),
        PreferencesPage::Font => page_font(ui, state),
        PreferencesPage::Startup => page_startup(ui, state),
        PreferencesPage::ExternalTools => page_external_tools(ui, state),
        PreferencesPage::ExplorerIntegration => page_explorer_integration(ui, state),
        PreferencesPage::Thumbnail => page_thumbnail(ui, state),
        PreferencesPage::Slideshow => page_slideshow(ui, state),
        PreferencesPage::Capture => page_capture(ui, state),
        PreferencesPage::CreativeLut => page_creative_lut(ui, state),
        PreferencesPage::MenuLayout => page_menu_layout(ui, state),
        PreferencesPage::Parallelism => page_parallelism(ui, state),
        PreferencesPage::Prefetch => page_prefetch(ui, state),
        PreferencesPage::GpuMemory => page_gpu_memory(ui, state),
        PreferencesPage::AiBackend => page_ai_backend(ui, state),
        PreferencesPage::Cache => page_cache(ui, state),
        PreferencesPage::Folder => page_folder(ui, state),
        PreferencesPage::Book => page_book(ui, state),
        PreferencesPage::DuplicateFiles => page_duplicate_files(ui, state),
        PreferencesPage::ExifDisplay => page_exif_display(ui, state, enter_pressed),
        PreferencesPage::SpreadMode => page_spread_mode(ui, state),
        PreferencesPage::PlaybackResume => page_playback_resume(ui, state),
        PreferencesPage::SusiePlugins => page_susie_plugins(ui, state),
        PreferencesPage::IndexerSpeed => page_indexer_speed(ui, state),
        PreferencesPage::TrayResidency => page_tray_residency(ui, state),
        PreferencesPage::Rating => page_rating(ui, state),
        PreferencesPage::UpdateCheck => page_update_check(ui, state),
        PreferencesPage::Video => page_video(ui, state),
        PreferencesPage::Vst3 => page_vst3(ui, state),
        PreferencesPage::EditingAddon => page_editing_addon(ui, state),
        PreferencesPage::Developer => page_developer(ui, state),
    }
}

// 個別ページ実装は `preferences/pages.rs` に分離。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn associated_app_candidates_come_only_from_current_handlers() {
        let handlers = vec![
            crate::open_with::AppHandler {
                display_name: "Photos".to_string(),
                handler_id: "photos.app".to_string(),
            },
            crate::open_with::AppHandler {
                display_name: "Same legacy target".to_string(),
                handler_id: r"c:\tools\VIEWER.exe".to_string(),
            },
            crate::open_with::AppHandler {
                display_name: "Paint".to_string(),
                handler_id: "Paint.App".to_string(),
            },
        ];

        let candidates = external_tool_launch_candidates(&handlers);

        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].display_name, "Photos");
        assert_eq!(candidates[1].display_name, "Same legacy target");
        assert_eq!(
            candidates[2],
            ExternalToolLaunchCandidate {
                display_name: "Paint".to_string(),
                launch: crate::external_tool::ExternalToolLaunch::Association {
                    handler_id: "Paint.App".to_string(),
                },
            }
        );
        assert!(external_tool_launch_candidates(&[]).is_empty());
    }

    #[test]
    fn settings_page_add_applies_context_menu_default_at_ten_and_eleven() {
        let mut settings = crate::settings::Settings::default();
        settings.external_tools = (1..=10)
            .map(|id| {
                let mut tool = crate::external_tool::ExternalTool::defaults_for_viewing();
                tool.id = crate::external_tool::ExternalToolId(id);
                tool
            })
            .collect();
        let mut state = PreferencesState::from_settings(
            &settings,
            crate::external_tool::LaunchTarget::None,
            None,
            false,
            0,
            0,
            0,
        );
        let mut duplicate = crate::external_tool::ExternalTool::defaults_for_viewing();
        duplicate.show_in_context_menu = false;

        state.add_external_tool(duplicate.clone());
        assert!(state.settings.external_tools[10].show_in_context_menu);

        state.add_external_tool(duplicate);
        assert!(!state.settings.external_tools[11].show_in_context_menu);
    }

    #[test]
    fn preferences_search_results_snapshot() {
        use egui_kittest::Harness;

        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(560.0, 320.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_width(ui.available_width());
                    let _ = draw_preference_search_results(ui, "GPU");
                });
            });
        harness.run();
        harness.snapshot("preferences_search_results");
    }

    #[test]
    fn preferences_parallelism_pdf_count_snapshot() {
        use egui_kittest::Harness;

        let mut state = PreferencesState::from_settings(
            &crate::settings::Settings::default(),
            crate::external_tool::LaunchTarget::None,
            None,
            false,
            0,
            0,
            0,
        );
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(560.0, 340.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_width(ui.available_width());
                    page_parallelism(ui, &mut state);
                });
            });
        harness.run();
        harness.snapshot("preferences_parallelism_pdf_count");
    }

    #[test]
    fn preferences_recycle_bin_delete_confirmation_snapshot() {
        use egui_kittest::Harness;

        let mut settings = crate::settings::Settings::default();
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(560.0, 230.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_width(ui.available_width());
                    draw_recycle_bin_delete_confirmation_setting(ui, &mut settings);
                });
            });
        harness.run();
        harness.snapshot("preferences_recycle_bin_delete_confirmation");
    }

    #[test]
    fn preferences_folder_edit_restore_snapshot() {
        use egui_kittest::Harness;

        let mut state = PreferencesState::from_settings(
            &crate::settings::Settings::default(),
            crate::external_tool::LaunchTarget::None,
            None,
            false,
            0,
            0,
            0,
        );
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(620.0, 680.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        page_folder(ui, &mut state);
                    });
                });
            });
        harness.run();
        harness.snapshot("preferences_folder_edit_restore");
    }

    #[test]
    fn preferences_viewer_notice_visibility_snapshot() {
        use egui_kittest::Harness;

        let mut state = PreferencesState::from_settings(
            &crate::settings::Settings::default(),
            crate::external_tool::LaunchTarget::None,
            None,
            false,
            0,
            0,
            0,
        );
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(560.0, 360.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        page_spread_mode(ui, &mut state);
                    });
                });
            });
        harness.run();
        harness.snapshot("preferences_viewer_notice_visibility");
    }

    fn assert_selection_bar_matches_details(settings: &crate::settings::Settings) {
        assert_eq!(
            settings.details_selection_bar_column_order,
            settings.details_column_order
        );
        assert_eq!(
            settings.details_selection_bar_column_widths,
            settings.details_column_widths
        );
        assert_eq!(
            settings.details_selection_bar_show_size,
            settings.details_show_size
        );
        assert_eq!(
            settings.details_selection_bar_show_created,
            settings.details_show_created
        );
        assert_eq!(
            settings.details_selection_bar_name_width_auto,
            settings.details_name_width_auto
        );
        assert_eq!(
            settings.details_selection_bar_name_width,
            settings.details_name_width
        );
    }

    #[test]
    fn preferences_scroll_generation_changes_only_on_page_switch() {
        let mut generation = 7;
        advance_preferences_scroll_generation(
            PreferencesPage::General,
            PreferencesPage::General,
            &mut generation,
        );
        assert_eq!(generation, 7);

        advance_preferences_scroll_generation(
            PreferencesPage::General,
            PreferencesPage::SpreadMode,
            &mut generation,
        );
        assert_eq!(generation, 8);
    }

    #[test]
    fn preferences_scroll_generation_advances_once_per_open_edge() {
        let mut open_last_frame = false;
        let mut sequence = 11;

        assert_eq!(
            advance_preferences_scroll_generation_on_open(
                false,
                &mut open_last_frame,
                &mut sequence
            ),
            None
        );
        assert_eq!(
            advance_preferences_scroll_generation_on_open(
                true,
                &mut open_last_frame,
                &mut sequence
            ),
            Some(12)
        );
        assert_eq!(
            advance_preferences_scroll_generation_on_open(
                true,
                &mut open_last_frame,
                &mut sequence
            ),
            None
        );
        assert_eq!(
            advance_preferences_scroll_generation_on_open(
                false,
                &mut open_last_frame,
                &mut sequence
            ),
            None
        );
        assert_eq!(
            advance_preferences_scroll_generation_on_open(
                true,
                &mut open_last_frame,
                &mut sequence
            ),
            Some(13)
        );
    }

    #[test]
    fn font_page_is_under_display_not_general() {
        let display = TREE
            .iter()
            .find(|category| category.label == "表示")
            .expect("display category should exist");
        assert!(display.children.contains(&PreferencesPage::Font));

        let general = TREE
            .iter()
            .find(|category| category.label == "全体設定")
            .expect("general category should exist");
        assert_eq!(general.page, Some(PreferencesPage::General));
        assert!(!general.children.contains(&PreferencesPage::Font));
    }

    #[test]
    fn preferences_commit_copies_live_details_columns_after_non_preference_merge() {
        let mut live = crate::settings::Settings::default();
        live.details_selection_bar_mode = crate::settings::DetailsSelectionBarMode::SameAsDetails;
        live.details_column_order = vec![
            crate::settings::DetailsColumnId::Kind,
            crate::settings::DetailsColumnId::Name,
        ];
        live.details_column_widths = vec![crate::settings::DetailsColumnWidth {
            column: crate::settings::DetailsColumnId::Kind,
            width: 215.0,
        }];
        live.details_show_size = false;
        live.details_show_created = true;
        live.details_name_width_auto = false;
        live.details_name_width = 287.0;
        live.details_selection_bar_column_order = vec![crate::settings::DetailsColumnId::Name];
        live.details_selection_bar_name_width = 99.0;

        // ダイアログを開いた時点の snapshot は古い A/C を持つが、モードだけ Dedicated に編集。
        let mut edited = crate::settings::Settings::default();
        edited.details_selection_bar_mode = crate::settings::DetailsSelectionBarMode::Dedicated;

        prepare_preferences_settings_for_commit(&mut edited, &mut live);

        assert_eq!(
            edited.details_selection_bar_mode,
            crate::settings::DetailsSelectionBarMode::Dedicated
        );
        assert_selection_bar_matches_details(&edited);
        assert_eq!(
            edited.details_selection_bar_column_order[0],
            crate::settings::DetailsColumnId::Kind,
            "copy must use the latest live A after non-preference fields are merged"
        );
    }

    #[test]
    fn preferences_commit_reentering_dedicated_mode_overwrites_previous_bar_columns() {
        let mut live = crate::settings::Settings::default();
        live.details_selection_bar_mode = crate::settings::DetailsSelectionBarMode::Dedicated;
        live.details_selection_bar_column_order = vec![crate::settings::DetailsColumnId::Name];
        live.details_selection_bar_name_width = 111.0;

        let mut shared = live.clone();
        shared.details_selection_bar_mode = crate::settings::DetailsSelectionBarMode::SameAsDetails;
        prepare_preferences_settings_for_commit(&mut shared, &mut live);
        assert_eq!(
            shared.details_selection_bar_column_order,
            vec![crate::settings::DetailsColumnId::Name],
            "leaving Dedicated must preserve C"
        );

        shared.details_column_order = vec![
            crate::settings::DetailsColumnId::Size,
            crate::settings::DetailsColumnId::Name,
        ];
        shared.details_column_widths = vec![crate::settings::DetailsColumnWidth {
            column: crate::settings::DetailsColumnId::Size,
            width: 246.0,
        }];
        shared.details_name_width_auto = false;
        shared.details_name_width = 333.0;
        shared.details_selection_bar_column_order = vec![crate::settings::DetailsColumnId::Kind];
        shared.details_selection_bar_name_width = 77.0;

        let mut dedicated_again = shared.clone();
        dedicated_again.details_selection_bar_mode =
            crate::settings::DetailsSelectionBarMode::Dedicated;
        prepare_preferences_settings_for_commit(&mut dedicated_again, &mut shared);

        assert_selection_bar_matches_details(&dedicated_again);
        assert_eq!(
            dedicated_again.details_selection_bar_column_order[0],
            crate::settings::DetailsColumnId::Size,
            "re-entering Dedicated must overwrite stale C from the latest A"
        );
        assert_eq!(dedicated_again.details_selection_bar_name_width, 333.0);
    }
}
