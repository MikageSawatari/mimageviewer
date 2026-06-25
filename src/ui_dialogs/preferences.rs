//! 統合環境設定ダイアログ。
//!
//! 左側にツリー、右側に設定パネルを配置した環境設定ウィンドウ。
//! OK / キャンセルで一時コピーを確定 or 破棄する。

use eframe::egui;
use std::collections::HashSet;

use crate::app::App;
use crate::ring_shortcut::{RightDragContext, RingShortcutContext};
use crate::settings::{Parallelism, Settings};

mod pages;
use self::pages::*;

fn pref_panel_scroll_style() -> egui::style::ScrollStyle {
    let mut scroll = egui::style::ScrollStyle::solid();
    scroll.bar_width = 10.0;
    scroll.bar_inner_margin = 8.0;
    scroll.bar_outer_margin = 2.0;
    scroll.foreground_color = true;
    scroll
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
    StartupFolder,
    ExplorerIntegration,
    Thumbnail,
    Slideshow,
    Capture,
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
    /// 履歴と復元 (読書履歴、読書/再生位置の復元)
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
    fn label(self) -> &'static str {
        match self {
            Self::General => "全体設定",
            Self::StartupFolder => "起動時に開く場所",
            Self::ExplorerIntegration => "エクスプローラ連携",
            Self::Thumbnail => "サムネイル",
            Self::Slideshow => "スライドショー",
            Self::Capture => "キャプチャ保存",
            Self::MenuLayout => "メニュー構成",
            Self::Parallelism => "並列読み込み",
            Self::Prefetch => "先読み",
            Self::GpuMemory => "GPUメモリ",
            Self::AiBackend => "AI バックエンド",
            Self::Cache => "キャッシュ",
            Self::Folder => "フォルダ",
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
            Self::RightDrag => "右ドラッグ",
            Self::MouseButtons => "マウス進む/戻る",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OperationOverviewEditor {
    RingShortcut(RingShortcutContext),
    MouseGesture(RightDragContext),
    MouseButtons(RingShortcutContext),
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
            PreferencesPage::StartupFolder,
            PreferencesPage::ExplorerIntegration,
            PreferencesPage::TrayResidency,
            PreferencesPage::UpdateCheck,
        ],
    },
    TreeCategory {
        label: "表示",
        page: None,
        children: &[
            PreferencesPage::Thumbnail,
            PreferencesPage::SpreadMode,
            PreferencesPage::Slideshow,
            PreferencesPage::Capture,
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

pub(crate) struct PreferencesState {
    /// 編集用の Settings 一時コピー
    pub settings: Settings,
    /// 現在選択中のページ
    pub selected: PreferencesPage,
    /// 展開中のカテゴリラベル
    pub expanded: HashSet<&'static str>,

    // ページ固有の一時状態
    pub manual_threads: usize,
    pub capture_output_dir_input: String,
    pub book_root_input: String,
    pub startup_folder_path_input: String,
    pub exif_add_tag_input: String,
    pub command_filter: String,
    pub command_selected: Option<crate::keymap::KeyAction>,
    pub command_edit_loaded_for: Option<crate::keymap::KeyAction>,
    pub command_chord_inputs: [String; 3],
    pub command_capture_slot: Option<usize>,
    pub command_edit_error: Option<String>,
    pub command_editor_dialog_open: bool,
    pub command_editor_source_chord: Option<crate::keymap::Chord>,
    pub operation_tab: OperationCustomizeTab,
    pub operation_settings_tab: OperationSettingsTab,
    pub operation_overview_editor: Option<OperationOverviewEditor>,
    pub operation_keyboard_context: Option<crate::keymap::KeyContext>,
    pub operation_keyboard_ctrl: bool,
    pub operation_keyboard_shift: bool,
    pub operation_keyboard_alt: bool,
    pub operation_ring_context: RingShortcutContext,
    pub operation_mouse_gesture_context: RightDragContext,
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
    /// 環境設定を開いた時点 / 削除後の読書履歴の記憶件数。
    pub reading_history_entry_count: usize,
    /// 読書履歴クリアを App 側へ伝える one-shot フラグ。
    pub reading_history_clear_requested: bool,
    /// 直近の読書履歴削除結果。
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
    pub(crate) fn from_settings(
        s: &Settings,
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
            expanded,
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
            command_selected: None,
            command_edit_loaded_for: None,
            command_chord_inputs: std::array::from_fn(|_| String::new()),
            command_capture_slot: None,
            command_edit_error: None,
            command_editor_dialog_open: false,
            command_editor_source_chord: None,
            operation_tab: OperationCustomizeTab::Settings,
            operation_settings_tab: OperationSettingsTab::Behavior,
            operation_overview_editor: None,
            operation_keyboard_context: None,
            operation_keyboard_ctrl: false,
            operation_keyboard_shift: false,
            operation_keyboard_alt: false,
            operation_ring_context: RingShortcutContext::Grid,
            operation_mouse_gesture_context: RightDragContext::Grid,
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

impl App {
    pub(crate) fn show_preferences_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_preferences {
            return;
        }

        // 初回: 一時コピーを作成
        if self.pref_state.is_none() {
            #[cfg_attr(not(windows), allow(unused_mut))]
            let mut new_state = PreferencesState::from_settings(
                &self.settings,
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
                egui::ScrollArea::vertical()
                    .id_salt("pref_tree")
                    .max_height(main_height)
                    .show(&mut left_ui, |ui| {
                        ui.set_min_width(tree_width - 12.0);
                        draw_tree(ui, state);
                    });

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
                egui::ScrollArea::vertical()
                    .id_salt("pref_panel")
                    .scroll_bar_visibility(
                        egui::containers::scroll_area::ScrollBarVisibility::AlwaysVisible,
                    )
                    .auto_shrink([false, false])
                    .max_height(main_height)
                    .show(&mut right_ui, |ui| {
                        ui.set_width(ui.available_width());
                        command_capture_waiting = state.command_capture_slot.is_some();
                        draw_page(ui, state, enter_pressed);
                    });

                // 全体の高さを確保
                ui.allocate_space(egui::vec2(available.x, main_height));

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // Esc でキャンセル (IME 変換中はスキップ)
                if escape_pressed && !command_capture_waiting {
                    cancel = true;
                }

                ui.horizontal(|ui| {
                    if ui.button("  OK  ").clicked() {
                        apply = true;
                        // (note: 「TRT 全エンジンビルド」ボタンのフラグは下のブロックで処理する)
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });

        if apply {
            if let Some(mut state) = self.pref_state.take() {
                let old_dup = (
                    self.settings.skip_zip_if_folder_exists,
                    self.settings.skip_image_if_video_exists,
                    self.settings.skip_duplicate_images,
                    self.settings.image_ext_priority.clone(),
                    self.settings.video_thumb_use_sidecar_image,
                    self.settings.archive_file_handling_resolved(),
                );
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
                state
                    .settings
                    .overwrite_non_preferences_from(&mut self.settings);

                state.settings.reading_history_limit = state
                    .settings
                    .reading_history_limit
                    .clamp(1, crate::reading_history_db::READING_HISTORY_LIMIT_MAX);
                self.settings = state.settings;
                if old_keymap_settings != self.settings.keymap {
                    let keymap = crate::keymap::Keymap::from_settings(&self.settings.keymap);
                    for warning in keymap.warnings() {
                        crate::logger::log(format!("[keymap] {warning}"));
                    }
                    keymap.install_global_native_video_shortcuts();
                    self.keymap = keymap;
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
                    self.settings.skip_image_if_video_exists,
                    self.settings.skip_duplicate_images,
                    self.settings.image_ext_priority.clone(),
                    self.settings.video_thumb_use_sidecar_image,
                    self.settings.archive_file_handling_resolved(),
                );
                if old_dup != new_dup {
                    self.reload_current_folder_preserving_override();
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
        } else if cancel || !open {
            self.pref_state = None;
            self.show_preferences = false;
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

        // 履歴と復元ページ: 読書履歴クリア (one-shot)。
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
                None => Err("読書履歴 DB を開けませんでした".to_string()),
            };
            if let Some(ps) = self.pref_state.as_mut() {
                match result {
                    Ok((deleted, remaining)) => {
                        ps.reading_history_entry_count = remaining;
                        ps.reading_history_clear_result =
                            Some(format!("読書履歴を {deleted} 件削除しました。"));
                    }
                    Err(err) => {
                        ps.reading_history_clear_result =
                            Some(format!("読書履歴の削除に失敗しました: {err}"));
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
        let ime_active = self.ime_input_active();
        let dialog_pos = ctx.content_rect().min + egui::vec2(46.0, 32.0);

        egui::Window::new("操作カスタマイズ")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos(dialog_pos)
            .default_size([1040.0, 720.0])
            .min_width(860.0)
            .min_height(560.0)
            .show(ctx, |ui| {
                let state = self.operation_customize_state.as_mut().unwrap();
                let available = ui.available_size();
                let bottom_height = 38.0;
                let main_height = (available.y - bottom_height - 14.0).max(360.0);

                draw_operation_customize_tabs(ui, state);
                ui.separator();

                let mut content_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(egui::Rect::from_min_size(
                            ui.cursor().min,
                            egui::vec2(available.x, main_height),
                        ))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                content_ui.spacing_mut().scroll = pref_panel_scroll_style();
                egui::ScrollArea::vertical()
                    .id_salt("operation_customize_panel")
                    .scroll_bar_visibility(
                        egui::containers::scroll_area::ScrollBarVisibility::AlwaysVisible,
                    )
                    .auto_shrink([false, false])
                    .max_height(main_height)
                    .show(&mut content_ui, |ui| {
                        ui.set_width(ui.available_width());
                        draw_operation_customize_page(ui, state, ime_active);
                    });
                ui.allocate_space(egui::vec2(available.x, main_height));

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
            draw_operation_command_editor_dialog(ctx, state, ime_active);
        }

        if apply {
            if let Some(state) = self.operation_customize_state.take() {
                self.apply_operation_customize_state(state);
            }
            self.show_operation_customize = false;
        } else if cancel || !open {
            self.operation_customize_state = None;
            self.show_operation_customize = false;
        }
    }

    fn apply_operation_customize_state(&mut self, state: PreferencesState) {
        let old_keymap_settings = self.settings.keymap.clone();
        let old_ring_shortcuts = self.settings.ring_shortcuts.clone();

        self.settings.keymap = state.settings.keymap;
        self.settings.ring_shortcuts = state.settings.ring_shortcuts;
        self.settings.ring_shortcuts.sanitize();

        if old_keymap_settings != self.settings.keymap {
            let keymap = crate::keymap::Keymap::from_settings(&self.settings.keymap);
            for warning in keymap.warnings() {
                crate::logger::log(format!("[keymap] {warning}"));
            }
            keymap.install_global_native_video_shortcuts();
            self.keymap = keymap;
            self.native_overlay_shortcut_help_cache = None;
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
            page_command_overview(ui, state);
        }
        OperationCustomizeTab::RingShortcut => {
            draw_ring_context_tabs(ui, state);
            ui.add_space(8.0);
            let context = state.operation_ring_context;
            page_ring_shortcut_assignments(ui, state, context);
        }
        OperationCustomizeTab::Keyboard => {
            draw_keyboard_context_tabs(ui, state);
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
            ui.small("ゲームパッド X+方向はリングショートカットと同じ割り当てを使います。X 単体で開くピッカーパネルの項目は固定です。");
            ui.add_space(8.0);
            let context = state.operation_ring_context;
            page_ring_shortcut_assignments(ui, state, context);
        }
    }
}

fn draw_keyboard_context_tabs(ui: &mut egui::Ui, state: &mut PreferencesState) {
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
        Some(KeyContext::Conceal),
        Some(KeyContext::Crop),
        Some(KeyContext::Text),
        Some(KeyContext::LocalAdjust),
    ];

    ui.horizontal_wrapped(|ui| {
        for &context in CONTEXTS {
            let label = context.map_or("すべて", KeyContext::description);
            if ui
                .selectable_label(state.operation_keyboard_context == context, label)
                .clicked()
            {
                state.operation_keyboard_context = context;
                state.command_capture_slot = None;
                state.command_edit_error = None;
            }
        }
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
                .selectable_label(state.operation_ring_context == context, context.label())
                .clicked()
            {
                state.operation_ring_context = context;
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
                } else if !is_expanded
                    && !cat.children.contains(&state.selected)
                    && let Some(&first_child) = cat.children.first()
                {
                    state.selected = first_child;
                }
            }

            // 子ページ
            if is_expanded {
                for &child in cat.children {
                    let selected = state.selected == child;
                    let text = format!("    {}", child.label());
                    if ui.selectable_label(selected, text).clicked() {
                        state.selected = child;
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
        PreferencesPage::StartupFolder => page_startup_folder(ui, state),
        PreferencesPage::ExplorerIntegration => page_explorer_integration(ui, state),
        PreferencesPage::Thumbnail => page_thumbnail(ui, state),
        PreferencesPage::Slideshow => page_slideshow(ui, state),
        PreferencesPage::Capture => page_capture(ui, state),
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
