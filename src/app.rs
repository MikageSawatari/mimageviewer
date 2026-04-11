use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};

use eframe::egui;

use crate::folder_tree::{
    is_apple_double, navigate_folder_with_skip, next_folder_dfs, prev_folder_dfs,
    walk_dirs_recursive, SUPPORTED_EXTENSIONS, SUPPORTED_VIDEO_EXTENSIONS,
};
use crate::fs_animation::{decode_apng_frames, decode_gif_frames, FsCacheEntry, FsLoadResult};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::thumb_loader::{
    build_and_save_one, compute_display_px, process_load_request, CacheDecision, LoadRequest,
    ThumbMsg,
};
use crate::ui_helpers::{
    draw_play_icon, natural_sort_key,
    open_external_player, truncate_name,
};

// -----------------------------------------------------------------------
// サブ構造体: サムネイル画質 A/B 比較ダイアログの状態
// -----------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct ThumbQualityState {
    pub show: bool,
    /// サンプル画像 (デコード済み、ダイアログを閉じるまで保持)
    pub sample: Option<image::DynamicImage>,
    /// サンプル画像のパス表示用
    pub sample_path: Option<PathBuf>,
    /// サンプル画像の元ファイルサイズ (bytes)
    pub sample_original_size: u64,
    /// パネル A: サイズ (long side px)
    pub a_size: u32,
    /// パネル A: 品質 (1–100)
    pub a_quality: u8,
    /// パネル A: プレビューテクスチャ
    pub a_texture: Option<egui::TextureHandle>,
    /// パネル A: エンコード後のバイト数
    pub a_bytes: usize,
    /// パネル B: サイズ
    pub b_size: u32,
    /// パネル B: 品質
    pub b_quality: u8,
    /// パネル B: プレビューテクスチャ
    pub b_texture: Option<egui::TextureHandle>,
    /// パネル B: エンコード後のバイト数
    pub b_bytes: usize,
    /// true = A/B 比較の全画面オーバーレイ表示中
    pub fullscreen: bool,
    /// 全画面 A/B 比較時の縦線位置（0.0=すべて B、1.0=すべて A、中央は 0.5）
    pub fs_divider: f32,
}

// -----------------------------------------------------------------------
// サブ構造体: キャッシュ作成バックグラウンドタスクの状態
// -----------------------------------------------------------------------

pub(crate) struct CacheCreatorState {
    pub show: bool,
    /// 各お気に入りのチェック状態（settings.favorites と同じ長さ）
    pub checked: Vec<bool>,
    /// 実行中フラグ（UI ボタンの有効/無効とポーリング制御）
    pub running: bool,
    /// カウントフェーズ中フラグ（total 未確定）
    pub counting: Arc<AtomicBool>,
    /// 対象フォルダ総数（Pass 1 完了後に確定）
    pub total: Arc<AtomicUsize>,
    /// 処理済みフォルダ数
    pub done: Arc<AtomicUsize>,
    /// キャッシュ容量 (バイト単位、累積加算)
    pub cache_size: Arc<AtomicU64>,
    /// キャンセルトークン
    pub cancel: Arc<AtomicBool>,
    /// 現在処理中のフォルダパス表示用
    pub current: Arc<Mutex<String>>,
    /// 完了シグナル（表示切替用）
    pub finished: Arc<AtomicBool>,
    /// 完了後のメッセージ
    pub result: Option<String>,
}

impl Default for CacheCreatorState {
    fn default() -> Self {
        Self {
            show: false,
            checked: Vec::new(),
            running: false,
            counting: Arc::new(AtomicBool::new(false)),
            total: Arc::new(AtomicUsize::new(0)),
            done: Arc::new(AtomicUsize::new(0)),
            cache_size: Arc::new(AtomicU64::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
            current: Arc::new(Mutex::new(String::new())),
            finished: Arc::new(AtomicBool::new(false)),
            result: None,
        }
    }
}

// -----------------------------------------------------------------------
// App
// -----------------------------------------------------------------------

pub struct App {
    pub(crate) address: String,
    pub(crate) current_folder: Option<PathBuf>,
    pub(crate) items: Vec<GridItem>,
    pub(crate) thumbnails: Vec<ThumbnailState>,
    pub(crate) selected: Option<usize>,
    pub(crate) settings: crate::settings::Settings,
    pub(crate) tx: mpsc::Sender<ThumbMsg>,
    pub(crate) rx: mpsc::Receiver<ThumbMsg>,
    /// フォルダ移動時に true にセットすると旧ロードタスクが中断する
    pub(crate) cancel_token: Arc<AtomicBool>,
    /// Phase 2b ワーカーが参照する現在の可視先頭アイテムインデックス
    /// UIスレッドが毎フレーム更新し、バックグラウンドワーカーが優先度に使う
    pub(crate) scroll_hint: Arc<AtomicUsize>,

    /// スクロールオフセット（行境界にスナップ済み）。自前管理する
    pub(crate) scroll_offset_y: f32,
    /// 前フレームのセル幅（ = avail_w / cols）
    pub(crate) last_cell_size: f32,
    /// 前フレームのセル高さ（ = last_cell_size * thumb_aspect.height_ratio()）
    pub(crate) last_cell_h: f32,
    /// 前フレームのビューポート高さ（カーソルキースクロールに使用）
    pub(crate) last_viewport_h: f32,
    /// true のとき選択セルが見えるようにオフセットを調整する
    pub(crate) scroll_to_selected: bool,

    /// ウィンドウ状態保存用：最後に確認した outer_rect（最小化・最大化時は更新しない）
    pub(crate) last_outer_rect: Option<egui::Rect>,
    /// 現在のウィンドウの DPI スケール（論理→物理変換に使用）
    pub(crate) last_pixels_per_point: f32,

    /// キャッシュ生成進捗：新規デコードが必要だった画像の総数
    pub(crate) cache_gen_total: usize,
    /// キャッシュ生成進捗：完了した枚数（rayon スレッドからアトミックに更新）
    pub(crate) cache_gen_done: Arc<AtomicUsize>,

    // ── 段階 B: ページ単位先読み / eviction ──────────────────────
    /// アイテム idx → 画像メタデータ (mtime, file_size)。フォルダ・動画は None
    pub(crate) image_metas: Vec<Option<(i64, i64)>>,
    /// 永続ワーカーがサムネイルを処理するためのキュー（UI からは push のみ）
    pub(crate) reload_queue: Option<Arc<Mutex<Vec<LoadRequest>>>>,
    /// ロード要求を送ったがまだ応答が来ていない idx 集合（重複要求防止）。
    /// 値は `true` ならアイドル時アップグレード要求、`false` なら通常の読み込み要求。
    pub(crate) requested: std::collections::HashMap<usize, bool>,
    /// 現在の keep range [start, end)。update_keep_range で毎フレーム更新
    pub(crate) keep_range: (usize, usize),

    // ── 進捗バー (段階 B/E の合算進捗表示) ─────────────────────
    /// 現フレームで検出された通常読み込みのピーク件数 (current が 0 でリセット)
    pub(crate) progress_normal_peak: usize,
    /// 現フレームで検出された高画質化 (アイドルアップグレード) のピーク件数
    pub(crate) progress_upgrade_peak: usize,

    /// 前フレームで選択中セルが描画された矩形 (スクリーン座標)。
    /// 選択情報オーバーレイをセル直下に配置するために使用。
    /// 選択セルがスクロール圏外だと None。
    pub(crate) selected_cell_rect: Option<egui::Rect>,

    // ── 段階 E: アイドル時の画質向上 ─────────────────────────────
    /// 前フレームでの scroll_offset_y（変化検知用）
    pub(crate) last_scroll_offset_y_tracked: f32,
    /// 最後にスクロールが動いた瞬間の時刻（アイドル検出用）
    pub(crate) last_scroll_change_time: std::time::Instant,
    /// UI とワーカー間で共有する現在の display_px (列数変更時に追従させる)
    /// update_keep_range_and_requests で毎フレーム更新される。
    pub(crate) display_px_shared: Arc<AtomicU32>,

    // ── 統計情報 (起動時から累計) ─────────────────────────────
    /// サムネイル読み込みの統計 (時間分布・サイズ分布・フォーマット)。
    /// ワーカースレッドから Arc 経由で更新され、UI スレッドが読み出す。
    pub(crate) stats: Arc<Mutex<crate::stats::ThumbStats>>,
    /// 統計ダイアログの表示フラグ
    pub(crate) show_stats_dialog: bool,

    // ── フルスクリーン表示・先読みキャッシュ ───────────────────────
    /// Some(idx) = フルスクリーン表示中（self.items のインデックス）
    pub(crate) fullscreen_idx: Option<usize>,
    /// 先読みキャッシュ: item_idx → ロード済みエントリ（静止画 or アニメーション）
    pub(crate) fs_cache: std::collections::HashMap<usize, FsCacheEntry>,
    /// 先読み中: item_idx → (キャンセルトークン, 受信チャネル)
    pub(crate) fs_pending: std::collections::HashMap<usize, (Arc<AtomicBool>, mpsc::Receiver<FsLoadResult>)>,

    // ── お気に入り編集ポップアップ ────────────────────────────────
    pub(crate) show_favorites_editor: bool,

    // ── お気に入り追加ダイアログ (名称入力) ───────────────────────
    pub(crate) show_fav_add_dialog: bool,
    pub(crate) fav_add_name_input: String,
    pub(crate) fav_add_target: Option<PathBuf>,

    // ── フォルダを開く ダイアログ (アドレスバーを隠したとき用) ───
    pub(crate) show_open_folder_dialog: bool,
    pub(crate) open_folder_input: String,
    /// フォルダを開くダイアログのエラーメッセージ
    pub(crate) open_folder_error: Option<String>,

    // ── 環境設定ポップアップ ─────────────────────────────────────
    pub(crate) show_preferences: bool,
    /// 環境設定ダイアログ内の一時的な並列度編集値（Manual時の数値）
    pub(crate) pref_manual_threads: usize,

    // ── ツールバー表示設定ポップアップ ───────────────────────────
    pub(crate) show_toolbar_settings: bool,

    // ── キャッシュ生成設定ポップアップ (段階 C) ──────────────────
    pub(crate) show_cache_policy_dialog: bool,

    // ── 複数選択 ──────────────────────────────────────────────────
    /// チェック済みアイテムの集合 (スペースキーで追加/削除)
    pub(crate) checked: std::collections::HashSet<usize>,

    // ── 右クリックコンテキストメニュー ─────────────────────────
    /// コンテキストメニューの対象アイテムインデックス
    pub(crate) context_menu_idx: Option<usize>,
    /// コンテキストメニューの表示座標 (右クリック時に記録)
    pub(crate) context_menu_pos: egui::Pos2,

    // ── 削除確認ダイアログ ───────────────────────────────────────
    pub(crate) show_delete_confirm: bool,
    /// 削除対象のファイルパスリスト
    pub(crate) delete_targets: Vec<(usize, PathBuf)>,

    // ── ペースト後のフォルダ再読み込みフラグ ──────────────────────
    pub(crate) pending_reload: bool,
    /// フォルダ読み込み後に選択するアイテム名（BS で親に戻るとき等）
    pub(crate) select_after_load: Option<String>,

    // ── 同名ファイル処理 ──────────────────────────────────────────
    pub(crate) video_thumb_overrides: std::collections::HashMap<String, PathBuf>,
    pub(crate) show_duplicate_settings: bool,
    /// 同名ファイル処理の一時編集状態
    pub(crate) dup_edit: Option<crate::ui_dialogs::duplicate_settings::DupEdit>,

    // ── 回転リセット確認ダイアログ ─────────────────────────────
    pub(crate) show_rotation_reset_confirm: bool,

    // ── スライドショー設定ポップアップ ─────────────────────────
    pub(crate) show_slideshow_settings: bool,
    /// スライドショー設定の一時編集値
    pub(crate) slideshow_edit_interval: Option<f32>,

    // ── EXIF 表示設定ポップアップ ──────────────────────────────
    pub(crate) show_exif_settings: bool,
    pub(crate) exif_add_tag_input: String,
    /// EXIF 設定の一時編集状態
    pub(crate) exif_edit_tags: Option<Vec<String>>,

    // ── キャッシュ管理ポップアップ ───────────────────────────────
    pub(crate) show_cache_manager: bool,
    /// キャッシュ管理の「◯日以上古い」入力値
    pub(crate) cache_manager_days: u32,
    /// 開いたときに取得するキャッシュ統計: (フォルダ数, 合計バイト)
    pub(crate) cache_manager_stats: Option<(usize, u64)>,
    /// 削除後の結果メッセージ
    pub(crate) cache_manager_result: Option<String>,

    // ── 最後に選択した画像 (サムネイル画質ダイアログで使用) ──
    pub(crate) last_selected_image_path: Option<PathBuf>,

    // ── サムネイル画質設定ダイアログ ───────────────────────────
    pub(crate) tq: ThumbQualityState,

    // ── キャッシュ作成ポップアップ ───────────────────────────────
    pub(crate) cc: CacheCreatorState,

    // ── メタデータパネル (AI + EXIF) ─────────────────────────────────
    /// フルスクリーンでメタデータパネルを表示するか
    pub(crate) show_metadata_panel: bool,
    /// AI メタデータキャッシュ: ファイルパス → パース結果 (None = メタデータなし)
    pub(crate) metadata_cache: std::collections::HashMap<PathBuf, Option<crate::png_metadata::AiMetadata>>,
    /// EXIF キャッシュ: ファイルパス → パース結果 (None = EXIF なし)
    pub(crate) exif_cache: std::collections::HashMap<PathBuf, Option<crate::exif_reader::ExifInfo>>,
    /// ComfyUI Raw Prompt JSON の展開状態
    pub(crate) metadata_show_raw_prompt: bool,
    /// ComfyUI Raw Workflow JSON の展開状態
    pub(crate) metadata_show_raw_workflow: bool,
    /// EXIF セクションの展開状態
    pub(crate) exif_sections_open: std::collections::HashMap<String, bool>,

    // ── アドレスバーフォーカス管理 ───────────────────────────────
    /// true のときアドレスバーが入力中 → キーショートカットを無効化
    pub(crate) address_has_focus: bool,

    // ── フォルダ履歴（スクロール位置・選択状態の復元用）────────────
    /// フォルダパス → (scroll_offset_y, selected_idx)
    pub(crate) folder_history: std::collections::HashMap<PathBuf, (f32, Option<usize>)>,

    // ── メタデータ検索 ────────────────────────────────────────────
    /// 検索バー表示フラグ
    pub(crate) show_search_bar: bool,
    /// 検索キーワード入力
    pub(crate) search_query: String,
    /// 検索結果フィルタ: Some = フィルタ中（表示するアイテムの元インデックス集合）
    pub(crate) search_filter: Option<std::collections::HashSet<usize>>,
    /// フィルタ適用後の表示アイテムインデックスリスト（フィルタなしなら全アイテム）。
    /// グリッド表示・フルスクリーンナビ・スライドショーで共有。
    pub(crate) visible_indices: Vec<usize>,
    /// 検索バーにフォーカスを当てるフラグ（1フレームだけ true）
    pub(crate) search_focus_request: bool,
    /// 検索バーの TextEdit がフォーカスを持っているか（毎フレーム更新）
    pub(crate) search_has_focus: bool,

    // ── 回転 DB ──────────────────────────────────────────────────
    /// 回転情報 DB (全体で 1 ファイル)
    pub(crate) rotation_db: Option<crate::rotation_db::RotationDb>,
    /// 現在フォルダのアイテムごとの回転キャッシュ (idx → Rotation)
    pub(crate) rotation_cache: std::collections::HashMap<usize, crate::rotation_db::Rotation>,

    // ── スライドショー ────────────────────────────────────────────
    /// スライドショー再生中フラグ
    pub(crate) slideshow_playing: bool,
    /// 次の画像に切り替える時刻
    pub(crate) slideshow_next_at: std::time::Instant,

    // ── フルスクリーンビューポート ─────────────────────────────
    /// フルスクリーンビューポートが一度でも作成されたか
    pub(crate) fs_viewport_created: bool,

    // ── 起動時の前回フォルダ復元フラグ ──────────────────────────
    pub(crate) initialized: bool,
}

impl Default for App {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            address: String::new(),
            current_folder: None,
            items: Vec::new(),
            thumbnails: Vec::new(),
            selected: None,
            settings: crate::settings::Settings::load(),
            tx,
            rx,
            cancel_token: Arc::new(AtomicBool::new(false)),
            scroll_hint: Arc::new(AtomicUsize::new(0)),
            scroll_offset_y: 0.0,
            last_cell_size: 200.0,
            last_cell_h: 200.0,
            last_viewport_h: 600.0,
            scroll_to_selected: false,
            last_outer_rect: None,
            last_pixels_per_point: 1.0,
            cache_gen_total: 0,
            cache_gen_done: Arc::new(AtomicUsize::new(0)),
            image_metas: Vec::new(),
            reload_queue: None,
            requested: std::collections::HashMap::new(),
            keep_range: (0, 0),
            progress_normal_peak: 0,
            progress_upgrade_peak: 0,
            selected_cell_rect: None,
            last_scroll_offset_y_tracked: 0.0,
            last_scroll_change_time: std::time::Instant::now(),
            display_px_shared: Arc::new(AtomicU32::new(512)),
            stats: Arc::new(Mutex::new(crate::stats::ThumbStats::new())),
            show_stats_dialog: false,
            fullscreen_idx: None,
            fs_cache: std::collections::HashMap::new(),
            fs_pending: std::collections::HashMap::new(),
            show_favorites_editor: false,
            show_fav_add_dialog: false,
            fav_add_name_input: String::new(),
            fav_add_target: None,
            show_open_folder_dialog: false,
            open_folder_input: String::new(),
            open_folder_error: None,
            show_preferences: false,
            pref_manual_threads: 4,
            show_toolbar_settings: false,
            show_cache_policy_dialog: false,
            checked: std::collections::HashSet::new(),
            context_menu_idx: None,
            context_menu_pos: egui::Pos2::ZERO,
            show_delete_confirm: false,
            delete_targets: Vec::new(),
            pending_reload: false,
            select_after_load: None,
            video_thumb_overrides: std::collections::HashMap::new(),
            show_duplicate_settings: false,
            dup_edit: None,
            show_rotation_reset_confirm: false,
            show_slideshow_settings: false,
            slideshow_edit_interval: None,
            show_exif_settings: false,
            exif_add_tag_input: String::new(),
            exif_edit_tags: None,
            show_cache_manager: false,
            cache_manager_days: 90,
            cache_manager_stats: None,
            cache_manager_result: None,
            last_selected_image_path: None,
            tq: ThumbQualityState {
                fs_divider: 0.5,
                a_size: 512,
                a_quality: 75,
                b_size: 512,
                b_quality: 85,
                ..Default::default()
            },
            cc: CacheCreatorState::default(),
            show_metadata_panel: false,
            metadata_cache: std::collections::HashMap::new(),
            exif_cache: std::collections::HashMap::new(),
            metadata_show_raw_prompt: false,
            metadata_show_raw_workflow: false,
            exif_sections_open: std::collections::HashMap::new(),
            address_has_focus: false,
            folder_history: std::collections::HashMap::new(),
            show_search_bar: false,
            search_query: String::new(),
            search_filter: None,
            visible_indices: Vec::new(),
            search_focus_request: false,
            search_has_focus: false,
            rotation_db: crate::rotation_db::RotationDb::open().ok(),
            rotation_cache: std::collections::HashMap::new(),
            slideshow_playing: false,
            slideshow_next_at: std::time::Instant::now(),
            fs_viewport_created: false,
            initialized: false,
        }
    }
}

impl App {
    /// いずれかのモーダルダイアログが開いているか。
    /// true の場合、キーボードショートカットやスクロールを無効化する。
    pub(crate) fn any_dialog_open(&self) -> bool {
        self.show_stats_dialog
            || self.show_favorites_editor
            || self.show_fav_add_dialog
            || self.show_open_folder_dialog
            || self.show_preferences
            || self.show_toolbar_settings
            || self.show_cache_policy_dialog
            || self.show_cache_manager
            || self.show_delete_confirm
            || self.show_duplicate_settings
            || self.show_rotation_reset_confirm
            || self.show_slideshow_settings
            || self.show_exif_settings
            || self.context_menu_idx.is_some()
    }

    pub fn load_folder(&mut self, path: PathBuf) {
        // タスク 3: パスが .zip ファイルなら ZIP を仮想フォルダとして開く
        if path.is_file() {
            let is_zip = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("zip"))
                .unwrap_or(false);
            if is_zip {
                self.load_zip_as_folder(path);
                return;
            }
        }

        crate::logger::log(format!("=== load_folder: {} ===", path.display()));

        // ── ディレクトリ走査（画像はメタデータも収集）────────────────
        let mut folders: Vec<GridItem> = Vec::new();
        // (path, is_video, mtime, file_size)
        let mut all_media: Vec<(PathBuf, bool, i64, i64)> = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    folders.push(GridItem::Folder(p));
                } else if is_apple_double(&p) {
                    // macOS/iPhone AppleDouble メタデータ — スキップ
                } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    let ext_lower = ext.to_ascii_lowercase();
                    let meta = entry.metadata().ok();
                    let mtime = meta.as_ref()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs() as i64);
                    let file_size = meta.map_or(0, |m| m.len() as i64);
                    if SUPPORTED_EXTENSIONS.contains(&ext_lower.as_str()) {
                        all_media.push((p, false, mtime, file_size));
                    } else if SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_lower.as_str()) {
                        all_media.push((p, true, mtime, file_size));
                    } else if ext_lower == "zip" {
                        // タスク 3: ZIP ファイルはフォルダのように扱う
                        folders.push(GridItem::Folder(p));
                    }
                }
            }
        }

        folders.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
        let sort = self.settings.sort_order;
        all_media.sort_by(|(a, _, a_mt, _), (b, _, b_mt, _)| {
            let an = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let bn = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
            sort.compare(an, *a_mt, bn, *b_mt, natural_sort_key)
        });

        // ── 同名ファイルフィルタ ─────────────────────────────────────
        self.video_thumb_overrides.clear();
        self.apply_duplicate_filters(&mut folders, &mut all_media);

        // items: フォルダ先頭 → メディア（画像・動画を名前順混在）
        let folder_count = folders.len();
        let mut items: Vec<GridItem> = folders;
        let mut image_metas: Vec<Option<(i64, i64)>> = vec![None; folder_count];
        let mut video_items: Vec<(usize, PathBuf, u64)> = Vec::new();

        for (offset, (p, is_video, mtime, file_size)) in all_media.iter().enumerate() {
            let item_idx = folder_count + offset;
            if *is_video {
                items.push(GridItem::Video(p.clone()));
                image_metas.push(None);
                video_items.push((item_idx, p.clone(), (*file_size).max(0) as u64));
            } else {
                items.push(GridItem::Image(p.clone()));
                image_metas.push(Some((*mtime, *file_size)));
            }
        }

        // 画像ファイル名集合 (カタログ掃除用キー)
        let existing_keys: std::collections::HashSet<String> = items
            .iter()
            .filter_map(|it| match it {
                GridItem::Image(p) => p.file_name()?.to_str().map(String::from),
                _ => None,
            })
            .collect();

        self.start_loading_items(path, items, image_metas, existing_keys, video_items);
    }

    /// タスク 3: ZIP ファイルを仮想フォルダとして開く。
    ///
    /// 内部の画像エントリを列挙し、サブディレクトリごとにグループ化してから
    /// 各グループに `ZipSeparator` を先頭に挿入する。グループ間はディレクトリ名順、
    /// グループ内は現在の sort_order でソートされる。
    pub fn load_zip_as_folder(&mut self, zip_path: PathBuf) {
        crate::logger::log(format!("=== load_zip_as_folder: {} ===", zip_path.display()));

        // ── ZIP エントリ列挙 ──
        let entries = match crate::zip_loader::enumerate_image_entries(&zip_path) {
            Ok(e) => e,
            Err(e) => {
                crate::logger::log(format!("  zip enumerate failed: {e}"));
                // 空状態で表示だけ更新
                self.start_loading_items(
                    zip_path,
                    Vec::new(),
                    Vec::new(),
                    std::collections::HashSet::new(),
                    Vec::new(),
                );
                return;
            }
        };
        crate::logger::log(format!("  zip: {} image entries", entries.len()));

        // ── サブディレクトリごとにグループ化 ──
        let mut groups: std::collections::BTreeMap<
            String,
            Vec<crate::zip_loader::ZipImageEntry>,
        > = std::collections::BTreeMap::new();
        for e in entries {
            let dir = crate::zip_loader::entry_dir(&e.entry_name).to_string();
            groups.entry(dir).or_default().push(e);
        }

        // 各グループ内を sort_order に従ってソート
        let sort = self.settings.sort_order;
        for (_, list) in groups.iter_mut() {
            list.sort_by(|a, b| {
                let an = crate::zip_loader::entry_basename(&a.entry_name);
                let bn = crate::zip_loader::entry_basename(&b.entry_name);
                sort.compare(an, a.mtime, bn, b.mtime, natural_sort_key)
            });
        }

        // ── items / image_metas を構築 ──
        // 複数グループがあれば各グループ先頭にセパレータを挿入する。
        // 単一グループ (ルート直下のみ) ならセパレータは不要。
        let insert_separators = groups.len() > 1;
        let mut items: Vec<GridItem> = Vec::new();
        let mut image_metas: Vec<Option<(i64, i64)>> = Vec::new();
        let mut existing_keys: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (dir, list) in groups {
            if insert_separators {
                let display = if dir.is_empty() {
                    "(ルート)".to_string()
                } else {
                    dir.clone()
                };
                items.push(GridItem::ZipSeparator { dir_display: display });
                image_metas.push(None);
            }
            for e in list {
                existing_keys.insert(e.entry_name.clone());
                items.push(GridItem::ZipImage {
                    zip_path: zip_path.clone(),
                    entry_name: e.entry_name,
                });
                image_metas.push(Some((e.mtime, e.uncompressed_size as i64)));
            }
        }

        // ZIP には動画は含まれない (Shell API がファイルパスを要求するため)
        self.start_loading_items(zip_path, items, image_metas, existing_keys, Vec::new());
    }

    /// load_folder と load_zip_as_folder の共通処理。
    ///
    /// 与えられた `items` / `image_metas` を新しい状態として設定し、
    /// 旧タスクをキャンセル → カタログを開く → 永続ワーカー + 動画スレッドを起動 →
    /// 履歴復元 → last_folder 保存 までを行う。
    fn start_loading_items(
        &mut self,
        source_path: PathBuf,
        items: Vec<GridItem>,
        image_metas: Vec<Option<(i64, i64)>>,
        catalog_existing_keys: std::collections::HashSet<String>,
        video_items: Vec<(usize, PathBuf, u64)>,
    ) {
        // ── 履歴保存 + 旧タスクキャンセル + 状態リセット ──
        if let Some(cur) = self.current_folder.clone() {
            self.folder_history.insert(cur, (self.scroll_offset_y, self.selected));
        }
        self.close_fullscreen();

        self.cancel_token.store(true, Ordering::Relaxed);
        crate::logger::log("  cancel_token -> true (old tasks will stop)");
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_token = Arc::clone(&cancel);

        let (tx, rx) = mpsc::channel();
        self.tx = tx.clone();
        self.rx = rx;

        self.current_folder = Some(source_path.clone());
        self.address = source_path.to_string_lossy().to_string();
        self.selected = None;
        self.scroll_offset_y = 0.0;
        self.scroll_to_selected = false;
        self.scroll_hint.store(0, Ordering::Relaxed);

        self.items = items;
        self.image_metas = image_metas;
        self.thumbnails = (0..self.items.len())
            .map(|_| ThumbnailState::Pending)
            .collect();
        self.requested.clear();
        self.keep_range = (0, 0);
        self.rebuild_visible_indices();
        self.metadata_cache.clear();
        self.exif_cache.clear();
        self.checked.clear();
        self.rotation_cache.clear();
        self.search_filter = None;
        self.search_query.clear();
        // visible_indices はアイテム設定後 (下の行) に再計算される

        // ── カタログを開く + cache_map ロード + 削除掃除 ──
        let cache_dir = crate::catalog::default_cache_dir();
        let catalog_arc: Option<Arc<crate::catalog::CatalogDb>> =
            crate::catalog::CatalogDb::open(&cache_dir, &source_path)
                .map_err(|e| crate::logger::log(format!("  catalog open failed: {e}")))
                .ok()
                .map(Arc::new);

        let cache_map: Arc<std::collections::HashMap<String, crate::catalog::CacheEntry>> =
            Arc::new(
                catalog_arc
                    .as_ref()
                    .and_then(|c| c.load_all().ok())
                    .unwrap_or_default(),
            );
        crate::logger::log(format!("  catalog: {} entries in DB", cache_map.len()));

        if let Some(ref cat) = catalog_arc {
            if let Err(e) = cat.delete_missing(&catalog_existing_keys) {
                crate::logger::log(format!("  catalog delete_missing failed: {e}"));
            }
        }

        // ── 進捗カウンタリセット + 共有 display_px 更新 ──
        self.cache_gen_total = 0;
        self.cache_gen_done = Arc::new(AtomicUsize::new(0));

        let initial_display_px = compute_display_px(
            self.last_cell_size,
            self.last_cell_h,
            self.last_pixels_per_point,
        );
        self.display_px_shared.store(initial_display_px, Ordering::Relaxed);
        crate::logger::log(format!(
            "  display_px = {initial_display_px}  cache_policy = {}",
            self.settings.cache_policy.label()
        ));

        // ── 永続ワーカー + (必要なら) 動画スレッドを起動 ──
        let reload_queue: Arc<Mutex<Vec<LoadRequest>>> = Arc::new(Mutex::new(Vec::new()));
        self.reload_queue = Some(Arc::clone(&reload_queue));

        self.spawn_thumbnail_workers(
            &tx,
            Arc::clone(&cancel),
            reload_queue,
            cache_map,
            catalog_arc,
        );
        if !video_items.is_empty() {
            self.spawn_video_thread(tx, cancel, video_items, self.video_thumb_overrides.clone());
        }

        // ── 履歴復元 + last_folder 保存 ──
        if let Some(&(scroll, sel)) = self.folder_history.get(&source_path) {
            self.scroll_offset_y = scroll;
            self.selected = sel;
            if sel.is_some() {
                self.scroll_to_selected = true;
            }
        } else if let Some(name) = self.select_after_load.take() {
            // 履歴がない場合のフォールバック: 指定名のアイテムを探して選択
            let name_lower = name.to_lowercase();
            if let Some(idx) = self.items.iter().position(|item| {
                item.name().to_lowercase() == name_lower
            }) {
                self.selected = Some(idx);
                self.scroll_to_selected = true;
            }
        }
        self.settings.last_folder = Some(source_path);
        self.settings.save();
    }

    /// 永続サムネイルワーカープールを `parallelism.thread_count()` 個 spawn する。
    /// 各ワーカーは `reload_queue` を `scroll_hint` 優先度で消費し続け、
    /// `cancel` が立つまで動作する。
    fn spawn_thumbnail_workers(
        &self,
        tx: &mpsc::Sender<ThumbMsg>,
        cancel: Arc<AtomicBool>,
        reload_queue: Arc<Mutex<Vec<LoadRequest>>>,
        cache_map: Arc<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        catalog_arc: Option<Arc<crate::catalog::CatalogDb>>,
    ) {
        let thumb_threads = self.settings.parallelism.thread_count();
        let thumb_px = self.settings.thumb_px;
        let thumb_quality = self.settings.thumb_quality;
        let cache_decision = CacheDecision::from_settings(&self.settings);
        let scroll_hint = Arc::clone(&self.scroll_hint);
        let display_px_shared = Arc::clone(&self.display_px_shared);
        let stats = Arc::clone(&self.stats);
        let cache_gen_done = Arc::clone(&self.cache_gen_done);

        crate::logger::log(format!("  spawning {thumb_threads} persistent workers"));

        for worker_idx in 0..thumb_threads {
            let queue = Arc::clone(&reload_queue);
            let tx_w = tx.clone();
            let cancel_w = Arc::clone(&cancel);
            let hint_w = Arc::clone(&scroll_hint);
            let cache_map_w = Arc::clone(&cache_map);
            let catalog_w = catalog_arc.clone();
            let done_w = Arc::clone(&cache_gen_done);
            let display_px_w = Arc::clone(&display_px_shared);
            let stats_w = Arc::clone(&stats);

            std::thread::spawn(move || {
                crate::logger::log(format!("  worker {worker_idx} started"));
                loop {
                    if cancel_w.load(Ordering::Relaxed) { break; }

                    // 可視先頭に最も近い要求を選ぶ
                    let req_opt: Option<LoadRequest> = {
                        let mut q = queue.lock().unwrap();
                        if q.is_empty() {
                            None
                        } else {
                            let vis = hint_w.load(Ordering::Relaxed);
                            let best = q
                                .iter()
                                .enumerate()
                                .min_by_key(|(_, r)| {
                                    let i = r.idx;
                                    if i < vis { vis - i } else { i - vis }
                                })
                                .map(|(pos, _)| pos)
                                .unwrap();
                            Some(q.swap_remove(best))
                        }
                    };

                    match req_opt {
                        Some(req) => {
                            if cancel_w.load(Ordering::Relaxed) { break; }
                            let display_px = display_px_w.load(Ordering::Relaxed);
                            process_load_request(
                                &req, &cache_map_w, &tx_w, catalog_w.as_deref(),
                                thumb_px, thumb_quality, display_px, cache_decision, &done_w,
                                &stats_w,
                            );
                        }
                        None => {
                            // キューが空: 短時間スリープしてキャンセル監視 + CPU 負荷軽減
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                    }
                }
                crate::logger::log(format!("  worker {worker_idx} stopped"));
            });
        }
    }

    /// 動画サムネイル取得スレッドを起動する。
    /// 各動画について Windows Shell API でサムネを取り出し、tx 経由で UI に送信する。
    fn spawn_video_thread(
        &self,
        tx: mpsc::Sender<ThumbMsg>,
        cancel: Arc<AtomicBool>,
        video_items: Vec<(usize, PathBuf, u64)>,
        thumb_overrides: std::collections::HashMap<String, PathBuf>,
    ) {
        let thumb_size = self.last_cell_size.max(256.0) as i32;
        let display_px = compute_display_px(
            self.last_cell_size,
            self.last_cell_h,
            self.last_pixels_per_point,
        );
        let stats = Arc::clone(&self.stats);

        std::thread::spawn(move || {
            for (idx, path, file_size) in video_items {
                if cancel.load(Ordering::Relaxed) { break; }

                // 同名画像がある場合はそれをサムネイルとして使用
                let stem = path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let ci = if let Some(img_path) = thumb_overrides.get(&stem) {
                    crate::logger::log(format!(
                        "  video thumb override: idx={idx} stem={stem} img={}",
                        img_path.display()
                    ));
                    crate::thumb_loader::decode_image_for_thumb(img_path, display_px)
                } else {
                    crate::video_thumb::get_video_thumbnail(&path, thumb_size)
                };
                crate::logger::log(format!(
                    "  video thumb: idx={idx} {}",
                    if ci.is_some() { "ok" } else { "FAIL" }
                ));
                // 統計: 動画件数 + サイズを記録 (成功のみ)
                if ci.is_some() {
                    if let Ok(mut s) = stats.lock() {
                        s.record_video(file_size);
                    }
                } else if let Ok(mut s) = stats.lock() {
                    s.record_failed();
                }
                // 動画 Shell API はアップグレード経路を持たないので from_cache = false
                // ピクセル寸法は取得できないため None
                let _ = tx.send((idx, ci, false, None));
            }
        });
    }

    fn poll_thumbnails(&mut self, ctx: &egui::Context) {
        let mut count = 0u32;
        let (keep_start, keep_end) = self.keep_range;
        while let Ok((i, color_image_opt, from_cache, source_dims)) = self.rx.try_recv() {
            if i < self.thumbnails.len() {
                // 結果を in-flight セットから除外
                self.requested.remove(&i);

                // 段階 B: 結果が届いたが既に keep_range 外に出ている場合、
                // テクスチャ生成を省略し Evicted のままにする (VRAM 節約)
                let in_keep_range = i >= keep_start && i < keep_end;

                match color_image_opt {
                    Some(color_image) => {
                        if in_keep_range {
                            // ColorImage の長辺ピクセル数を記録しておく (段階 E)
                            let [w, h] = color_image.size;
                            let rendered_at_px = w.max(h) as u32;
                            let handle = ctx.load_texture(
                                format!("thumb_{i}"),
                                color_image,
                                egui::TextureOptions::LINEAR,
                            );
                            // 段階 E: from_cache と rendered_at_px を記録し、
                            // アイドル時のアップグレード対象か判定する
                            // source_dims は選択オーバーレイで使う
                            self.thumbnails[i] = ThumbnailState::Loaded {
                                tex: handle,
                                from_cache,
                                rendered_at_px,
                                source_dims,
                            };
                        } else {
                            // 範囲外: ColorImage を drop し Evicted にしておく
                            self.thumbnails[i] = ThumbnailState::Evicted;
                        }
                    }
                    None => {
                        self.thumbnails[i] = ThumbnailState::Failed;
                    }
                }
                count += 1;
            } else {
                // idx がアイテム範囲外 (旧フォルダの結果) は単純に捨てる
                self.requested.remove(&i);
            }
        }
        if count > 0 {
            crate::logger::log(format!("  [main] poll_thumbnails: received {count} thumbnail(s)"));
            ctx.request_repaint();
        }
    }

    /// 段階 B: ページ単位先読み + eviction のメインロジック。
    /// 段階 D: VRAM 安全ネット (上限超過時に keep_range を縮小)。
    ///
    /// 毎フレーム呼ぶ想定。現在のスクロール位置から keep_range を算出し、
    /// 範囲外の Loaded を Evicted 化し、範囲内の Pending/Evicted を reload_queue に push する。
    fn update_keep_range_and_requests(&mut self) {
        let total = self.items.len();
        if total == 0 {
            self.keep_range = (0, 0);
            return;
        }

        // 毎フレーム display_px を更新してワーカーに追従させる
        // (列数変更やウィンドウリサイズに対応)
        let current_display_px = compute_display_px(
            self.last_cell_size,
            self.last_cell_h,
            self.last_pixels_per_point,
        );
        self.display_px_shared
            .store(current_display_px, Ordering::Relaxed);

        let cols = self.settings.grid_cols.max(1);
        let cell_h = self.last_cell_h.max(1.0);
        let viewport_h = self.last_viewport_h.max(cell_h);

        let rows_per_page = (viewport_h / cell_h).ceil() as usize;
        let items_per_page = (rows_per_page * cols).max(1);
        // visible_indices 上の位置で keep_range を計算し、raw index に変換する。
        // これにより検索フィルタ中でも正しいサムネイルがロードされる。
        let vis_count = self.visible_indices.len();
        let vis_first = (self.scroll_offset_y / cell_h) as usize * cols;

        let prev_pages = self.settings.thumb_prev_pages as usize;
        let next_pages = self.settings.thumb_next_pages as usize;

        let vis_keep_start = vis_first.saturating_sub(prev_pages * items_per_page);
        let vis_keep_end = vis_first
            .saturating_add((1 + next_pages) * items_per_page)
            .min(vis_count);

        // visible_indices 経由で raw index の範囲を求める
        let mut keep_start = self
            .visible_indices
            .get(vis_keep_start)
            .copied()
            .unwrap_or(0);
        let mut keep_end = self
            .visible_indices
            .get(vis_keep_end.saturating_sub(1))
            .copied()
            .map(|i| i + 1)
            .unwrap_or(total)
            .min(total);

        // ── 段階 D: VRAM 安全ネット ──────────────────────────────────
        // display_px から 1 枚あたりの推定バイト数を算出し、cap を超えそうなら
        // keep_range を vis_first 中心に縮小する (前方 2/3 優先、後方 1/3)
        // 上限は "プライマリ GPU VRAM × 設定 %" (0 で無制限)
        let cap_percent = self.settings.thumb_vram_cap_percent;
        if cap_percent > 0 {
            let est_per_thumb: u64 = (current_display_px as u64)
                .saturating_mul(current_display_px as u64)
                .saturating_mul(4);
            let cap_bytes = crate::gpu_info::vram_cap_from_percent(cap_percent);
            if est_per_thumb > 0 {
                let max_items = (cap_bytes / est_per_thumb).max(1) as usize;
                let desired = keep_end.saturating_sub(keep_start);
                if max_items < desired {
                    // 上限超過: keep_range を縮小
                    let half_back = max_items / 3;
                    let half_forward = max_items - half_back;
                    let new_start = vis_first.saturating_sub(half_back);
                    let new_end = vis_first.saturating_add(half_forward).min(total);
                    crate::logger::log(format!(
                        "  VRAM cap hit: desired={desired} max_items={max_items} \
                         (display_px={current_display_px} est/thumb={} MB cap={} MB @ {}%)",
                        est_per_thumb / (1024 * 1024),
                        cap_bytes / (1024 * 1024),
                        cap_percent,
                    ));
                    keep_start = new_start;
                    keep_end = new_end;
                }
            }
        }

        self.keep_range = (keep_start, keep_end);

        // (1) 範囲外の Loaded を Evicted にする (TextureHandle を drop)
        //     動画サムネイルは一度ロードしたら維持する (別パスのため再要求できない)
        for i in 0..total {
            if i >= keep_start && i < keep_end {
                continue;
            }
            if matches!(self.items.get(i), Some(GridItem::Video(_))) {
                continue;
            }
            if matches!(self.thumbnails[i], ThumbnailState::Loaded { .. }) {
                self.thumbnails[i] = ThumbnailState::Evicted;
            }
        }

        // (2) 範囲内の Pending/Evicted を reload_queue に push
        let Some(queue) = self.reload_queue.clone() else { return; };
        let mut new_requests: Vec<LoadRequest> = Vec::new();
        for i in keep_start..keep_end {
            if self.requested.contains_key(&i) {
                continue;
            }
            let need_load = matches!(
                self.thumbnails[i],
                ThumbnailState::Pending | ThumbnailState::Evicted
            );
            if !need_load {
                continue;
            }
            // 画像 / ZIP 内画像のみ要求。フォルダ・動画・セパレータはスキップ
            let Some((mtime, file_size)) = self.image_metas.get(i).copied().flatten() else {
                continue;
            };
            let Some(req) = self.items.get(i).and_then(|item| {
                make_load_request(item, i, mtime, file_size, false)
            }) else {
                continue;
            };
            new_requests.push(req);
        }
        if !new_requests.is_empty() {
            let mut q = queue.lock().unwrap();
            for r in new_requests {
                // false = 通常読み込み要求
                self.requested.insert(r.idx, false);
                q.push(r);
            }
        }

        // (3) 段階 E: アイドル時の画質アップグレード
        self.enqueue_idle_upgrades(keep_start, keep_end);

        // (4) 進捗ピーク値の更新 (プログレスバー表示用)
        self.update_progress_peaks();
    }

    /// 段階 E: アイドル時に画質アップグレードの要求を投入する。
    ///
    /// 発動条件:
    /// - 設定 `thumb_idle_upgrade` が有効
    /// - スクロールが一定時間 (500 ms) 停止している
    /// - `reload_queue` が空で `requested` も空 (他の作業が全て終わっている)
    ///
    /// アップグレード対象:
    /// 1. `Loaded { from_cache: true }` — キャッシュ (WebP q=75) 由来で画質劣化
    /// 2. `Loaded { rendered_at_px < current_display_px * 0.8 }` —
    ///    列数変更などで現在のセルサイズより 20% 以上小さい解像度で生成されている
    ///
    /// `keep_range` 内の該当セルを最大 `BATCH` 件ずつ、`skip_cache = true` の
    /// LoadRequest として push する。スクロール優先度付きの worker が visible
    /// 側から先に処理する。
    fn enqueue_idle_upgrades(&mut self, keep_start: usize, keep_end: usize) {
        const SCROLL_IDLE_SECS: f64 = 0.5;

        if !self.settings.thumb_idle_upgrade {
            return;
        }

        // スクロール変化の検出
        if (self.scroll_offset_y - self.last_scroll_offset_y_tracked).abs() > 0.5 {
            self.last_scroll_change_time = std::time::Instant::now();
            self.last_scroll_offset_y_tracked = self.scroll_offset_y;
        }
        let scroll_idle =
            self.last_scroll_change_time.elapsed().as_secs_f64() >= SCROLL_IDLE_SECS;
        if !scroll_idle {
            return;
        }

        // キューと in-flight が両方空のときだけ走らせる
        if !self.requested.is_empty() {
            return;
        }
        let Some(queue) = self.reload_queue.clone() else { return; };
        {
            let q = queue.lock().unwrap();
            if !q.is_empty() {
                return;
            }
        }

        // 現在の display_px (アイドル判定とサイズ比較に使用)
        let current_display_px = self.display_px_shared.load(Ordering::Relaxed);

        // 候補集め: keep_range 内で from_cache=true or 解像度不足のものを全件
        //
        // ※ 以前は BATCH=4 で小分け push していたが、進捗バーが「0/4 → 4/4 → 消える」
        //    を繰り返すちらつき現象が発生していた。現在は keep_range 内の全候補を
        //    一度に push し、進捗バーが 1 本だけ綺麗に伸びるようにする。
        //    - スクロール時は scroll_idle ガードで新規 push されない
        //    - 通常ロードが必要なら requested ガードで先送り
        //    - フォルダ切替は cancel_token で全停止
        //    なので大量 push しても害は無い (古い結果は poll_thumbnails で
        //    keep_range 外なら自動破棄される)。
        let mut upgrade_reqs: Vec<LoadRequest> = Vec::new();
        for i in keep_start..keep_end {
            let needs_upgrade = match self.thumbnails.get(i) {
                Some(ThumbnailState::Loaded {
                    from_cache,
                    rendered_at_px,
                    ..
                }) => {
                    // 1. キャッシュ由来 (品質アップグレード)
                    // 2. 現在のセルに対して解像度不足 (rendered < current * 0.8)
                    //    u32 オーバーフロー対策で u64 で比較
                    *from_cache
                        || (*rendered_at_px as u64) * 5
                            < (current_display_px as u64) * 4
                }
                _ => false,
            };
            if !needs_upgrade {
                continue;
            }
            let Some((mtime, file_size)) = self.image_metas.get(i).copied().flatten() else {
                continue;
            };
            let Some(req) = self.items.get(i).and_then(|item| {
                make_load_request(item, i, mtime, file_size, true)
            }) else {
                continue;
            };
            upgrade_reqs.push(req);
        }

        if upgrade_reqs.is_empty() {
            return;
        }

        crate::logger::log(format!(
            "  idle upgrade: queued {} items (display_px={})",
            upgrade_reqs.len(),
            current_display_px,
        ));
        let mut q = queue.lock().unwrap();
        for r in upgrade_reqs {
            // true = 高画質化要求
            self.requested.insert(r.idx, true);
            q.push(r);
        }
    }

    /// in-flight + キュー内の通常/アップグレード件数を返す。
    fn count_pending(&self) -> (usize, usize) {
        let (mut in_normal, mut in_upgrade) = (0usize, 0usize);
        for &is_upgrade in self.requested.values() {
            if is_upgrade { in_upgrade += 1; } else { in_normal += 1; }
        }
        let (q_normal, q_upgrade) = if let Some(queue) = &self.reload_queue {
            let q = queue.lock().unwrap();
            let upgrade = q.iter().filter(|r| r.skip_cache).count();
            (q.len() - upgrade, upgrade)
        } else {
            (0, 0)
        };
        (in_normal + q_normal, in_upgrade + q_upgrade)
    }

    /// 現在の要求状況からプログレスバーのピーク値を更新する。
    fn update_progress_peaks(&mut self) {
        let (cur_normal, cur_upgrade) = self.count_pending();

        if cur_normal == 0 {
            self.progress_normal_peak = 0;
        } else if cur_normal > self.progress_normal_peak {
            self.progress_normal_peak = cur_normal;
        }
        if cur_upgrade == 0 {
            self.progress_upgrade_peak = 0;
        } else if cur_upgrade > self.progress_upgrade_peak {
            self.progress_upgrade_peak = cur_upgrade;
        }
    }

    /// プログレスバーの現在値を計算して返す。
    /// `(normal (cur, peak), upgrade (cur, peak))`
    pub(crate) fn progress_snapshot(&self) -> ((usize, usize), (usize, usize)) {
        let (cur_normal, cur_upgrade) = self.count_pending();
        (
            (cur_normal, self.progress_normal_peak),
            (cur_upgrade, self.progress_upgrade_peak),
        )
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        // ウィンドウにフォーカスがない場合はキー入力を無視
        let has_focus = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if !has_focus {
            return None;
        }
        // フルスクリーン、ダイアログ、テキスト入力中はショートカットを無効化
        if self.fullscreen_idx.is_some()
            || self.any_dialog_open()
            || self.address_has_focus
            || self.search_has_focus
        {
            return None;
        }

        let cols = self.settings.grid_cols.max(1);

        // Ctrl の状態判定。AutoHotKey 等の外部ツールが Ctrl+矢印を送信する場合、
        // Ctrl と矢印が別フレームで届くことがある。直前フレームの Ctrl 押下も
        // 考慮するため、egui の全イベントから Ctrl 修飾子を探す。
        let ctrl_held = ctx.input(|i| {
            // 現在のフレームで Ctrl が押されている
            if i.modifiers.ctrl {
                return true;
            }
            // イベントの中に Ctrl 修飾子付きのキーイベントがあるか
            i.events.iter().any(|e| match e {
                egui::Event::Key { modifiers, .. } => modifiers.ctrl,
                _ => false,
            })
        });

        let (right, left, down, up, enter, backspace, _ctrl_up_raw, _ctrl_down_raw,
             home, end, page_up, page_down, space, key_r, key_l) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Backspace),
                i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowUp),
                i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
                i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::PageDown),
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::R),
                i.key_pressed(egui::Key::L),
            )
        });

        // Ctrl+矢印: modifiers.ctrl に加え ctrl_held (key_down) でも判定
        let ctrl_up = ctrl_held && up;
        let ctrl_down = ctrl_held && down;

        let vi = &self.visible_indices;
        let vi_len = vi.len();

        if vi_len > 0 {
            let sel = self.selected.unwrap_or_else(|| vi.first().copied().unwrap_or(0));
            // visible_indices 内での現在位置
            let vis_pos = vi.iter().position(|&i| i == sel).unwrap_or(0);
            let cell_h = self.last_cell_h.max(1.0);
            let visible_rows = (self.last_viewport_h / cell_h).floor() as usize;
            let page_items = visible_rows.max(1) * cols;

            // visible_indices 上で移動し、raw index に変換
            // Ctrl+矢印はフォルダ移動に使うので、通常カーソル移動から除外
            let new_vis_pos = if right && !ctrl_held {
                Some((vis_pos + 1).min(vi_len - 1))
            } else if left && !ctrl_held {
                Some(vis_pos.saturating_sub(1))
            } else if down && !ctrl_down {
                Some((vis_pos + cols).min(vi_len - 1))
            } else if up && !ctrl_up {
                Some(vis_pos.saturating_sub(cols))
            } else if home {
                Some(0)
            } else if end {
                Some(vi_len - 1)
            } else if page_down {
                Some((vis_pos + page_items).min(vi_len - 1))
            } else if page_up {
                Some(vis_pos.saturating_sub(page_items))
            } else {
                None
            };

            let shift = ctx.input(|i| i.modifiers.shift);
            let new_sel = new_vis_pos.and_then(|vp| vi.get(vp).copied());

            if let Some(s) = new_sel {
                // Shift+カーソル: 移動元から移動先までの画像をチェックに追加
                if shift && new_vis_pos.is_some() {
                    let old_pos = vis_pos;
                    let new_pos = new_vis_pos.unwrap();
                    let (start, end) = if old_pos <= new_pos {
                        (old_pos, new_pos)
                    } else {
                        (new_pos, old_pos)
                    };
                    for vp in start..=end {
                        if let Some(&idx) = vi.get(vp) {
                            match self.items.get(idx) {
                                Some(GridItem::Image(_))
                                | Some(GridItem::Video(_))
                                | Some(GridItem::ZipImage { .. }) => {
                                    self.checked.insert(idx);
                                }
                                _ => {}
                            }
                        }
                    }
                }

                self.selected = Some(s);
                self.scroll_to_selected = true;
                self.update_last_selected_image();
            }

            // スペースキー: チェック ON/OFF
            if space {
                if let Some(idx) = self.selected {
                    if self.checked.contains(&idx) {
                        self.checked.remove(&idx);
                    } else {
                        // フォルダ・セパレータはチェック対象外
                        match self.items.get(idx) {
                            Some(GridItem::Image(_))
                            | Some(GridItem::Video(_))
                            | Some(GridItem::ZipImage { .. }) => {
                                self.checked.insert(idx);
                            }
                            _ => {}
                        }
                    }
                }
            }

            // L/R: 選択画像を回転
            if key_r {
                if let Some(idx) = self.selected {
                    self.rotate_image_cw(idx);
                }
            }
            if key_l {
                if let Some(idx) = self.selected {
                    self.rotate_image_ccw(idx);
                }
            }

            if enter {
                if let Some(idx) = self.selected {
                    match self.items.get(idx) {
                        Some(GridItem::Folder(p)) => return Some(p.clone()),
                        Some(GridItem::Image(_))
                        | Some(GridItem::ZipImage { .. })
                        | Some(GridItem::ZipSeparator { .. }) => self.open_fullscreen(idx),
                        Some(GridItem::Video(p)) => {
                            let vp = p.clone();
                            open_external_player(&vp);
                        }
                        None => {}
                    }
                }
            }
        }

        // BS: 親フォルダへ
        if backspace {
            if let Some(ref cur) = self.current_folder.clone() {
                if let Some(parent) = cur.parent() {
                    // 親に戻ったとき、元のフォルダ名を選択するようにヒントを設定
                    self.select_after_load = cur
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string());
                    return Some(parent.to_path_buf());
                }
            }
        }

        // Ctrl+↓: 深さ優先で次のフォルダへ（画像なしはスキップ）
        if ctrl_down {
            if let Some(ref cur) = self.current_folder.clone() {
                if let Some(next) = navigate_folder_with_skip(cur, next_folder_dfs, self.settings.folder_skip_limit) {
                    return Some(next);
                }
            }
        }

        // Ctrl+↑: 深さ優先で前のフォルダへ（画像なしはスキップ）
        if ctrl_up {
            if let Some(ref cur) = self.current_folder.clone() {
                if let Some(prev) = navigate_folder_with_skip(cur, prev_folder_dfs, self.settings.folder_skip_limit) {
                    return Some(prev);
                }
            }
        }

        None
    }

    /// マウスホイールイベントを消費し、行単位でスナップしたオフセットに変換する。
    /// Ctrl+ホイールの場合はグリッド列数を変更する。
    fn process_scroll(&mut self, ctx: &egui::Context) {
        // ダイアログやフルスクリーン表示中はスクロールを消費しない
        // (ダイアログ内の ScrollArea が正しく動くようにする)
        if self.fullscreen_idx.is_some() || self.any_dialog_open() {
            return;
        }

        let cell_h = self.last_cell_h.max(1.0);

        // マウスホイールイベントだけを取り出し、egui には渡さない
        let (scroll_delta_y, ctrl) = ctx.input(|i| (i.raw_scroll_delta.y, i.modifiers.ctrl));
        if scroll_delta_y.abs() > 0.5 {
            ctx.input_mut(|i| {
                i.raw_scroll_delta = egui::Vec2::ZERO;
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                // MouseWheel イベントも消費
                i.events
                    .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
            });

            if ctrl {
                // Ctrl+ホイール: 列数を増減（2〜10 の範囲）
                let delta = -scroll_delta_y.signum() as i32;
                let new_cols = (self.settings.grid_cols as i32 + delta).clamp(2, 10) as usize;
                if new_cols != self.settings.grid_cols {
                    self.settings.grid_cols = new_cols;
                    self.settings.save();
                }
            } else {
                // 上スクロール(delta>0) → オフセット減、下スクロール(delta<0) → オフセット増
                let direction = -scroll_delta_y.signum();
                self.scroll_offset_y =
                    (self.scroll_offset_y + direction * cell_h).max(0.0);
                // 行境界にスナップ
                self.scroll_offset_y =
                    (self.scroll_offset_y / cell_h).round() * cell_h;
            }
        }
    }

    /// カーソルキー移動後、選択行がビューポートに収まるようオフセットを調整する
    pub(crate) fn apply_scroll_to_selected(&mut self, cols: usize, cell_h: f32) {
        let sel = match self.selected {
            Some(s) => s,
            None => return,
        };
        // フィルタ中は visible_indices 内での位置から行を計算する
        let vis_pos = self
            .visible_indices
            .iter()
            .position(|&i| i == sel)
            .unwrap_or(sel);
        let row = vis_pos / cols;
        let row_top = row as f32 * cell_h;
        let row_bottom = row_top + cell_h;
        let vp_top = self.scroll_offset_y;
        let vp_bottom = self.scroll_offset_y + self.last_viewport_h;

        if row_top < vp_top {
            // 選択行が上に隠れている → 選択行が最上行になるようスクロール
            self.scroll_offset_y = row_top;
        } else if row_bottom > vp_bottom {
            // 選択行が下に隠れている → 選択行が最下行になるようスクロール
            self.scroll_offset_y =
                (row_bottom - self.last_viewport_h).max(0.0);
            // 行境界にスナップ
            self.scroll_offset_y =
                (self.scroll_offset_y / cell_h).ceil() * cell_h;
        }
    }

    // -----------------------------------------------------------------------
    // フルスクリーン表示
    // -----------------------------------------------------------------------

    /// フルスクリーン表示を開始する。
    /// キャッシュ済みなら即座に表示し、そうでなければ読み込みを開始する。
    /// 動画アイテムの場合はサムネイル＋再生ボタンを表示するだけで読み込みは不要。
    pub fn open_fullscreen(&mut self, idx: usize) {
        crate::logger::log(format!("=== open_fullscreen: idx={idx} ==="));
        self.fullscreen_idx = Some(idx);

        match self.items.get(idx) {
            Some(GridItem::Image(_)) | Some(GridItem::ZipImage { .. }) => {
                if self.fs_cache.contains_key(&idx) {
                    crate::logger::log(format!("  cache hit idx={idx} → instant display"));
                } else if !self.fs_pending.contains_key(&idx) {
                    self.start_fs_load(idx);
                }
                self.update_prefetch_window(idx);
            }
            Some(GridItem::Video(_)) => {
                // 動画はサムネイル + 再生ボタンのみ。高解像度読み込み不要。
                crate::logger::log(format!("  video idx={idx} → play button mode"));
            }
            Some(GridItem::ZipSeparator { dir_display }) => {
                // セパレータはテキスト表示のみ (デコード不要)
                crate::logger::log(format!(
                    "  zip separator idx={idx} → title mode: {dir_display}"
                ));
            }
            _ => {}
        }

        // AI メタデータ読み込み (PNG ヘッダのみ読むので高速・同期)
        self.ensure_metadata_loaded(idx);
    }

    /// 指定 idx の AI メタデータと EXIF がキャッシュに無ければ読み込む。
    fn ensure_metadata_loaded(&mut self, idx: usize) {
        let path = match self.items.get(idx) {
            Some(GridItem::Image(p)) => p.clone(),
            Some(GridItem::ZipImage { zip_path, .. }) => zip_path.clone(),
            _ => return,
        };

        // AI メタデータ (PNG のみ)
        if !self.metadata_cache.contains_key(&path) {
            let meta = match self.items.get(idx) {
                Some(GridItem::Image(p)) => crate::png_metadata::extract_metadata(p),
                Some(GridItem::ZipImage { zip_path, entry_name }) => {
                    if let Ok(bytes) = crate::zip_loader::read_entry_bytes(zip_path, entry_name) {
                        crate::png_metadata::extract_metadata_from_bytes(&bytes)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            self.metadata_cache.insert(path.clone(), meta);
        }

        // EXIF (JPEG, PNG, TIFF 等)
        if !self.exif_cache.contains_key(&path) {
            let hidden = &self.settings.exif_hidden_tags;
            let exif = match self.items.get(idx) {
                Some(GridItem::Image(p)) => crate::exif_reader::read_exif(p, hidden),
                Some(GridItem::ZipImage { zip_path, entry_name }) => {
                    if let Ok(bytes) = crate::zip_loader::read_entry_bytes(zip_path, entry_name) {
                        crate::exif_reader::read_exif_from_bytes(&bytes, hidden)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            self.exif_cache.insert(path, exif);
        }
    }

    /// 同名ファイルフィルタを適用する。
    /// - ZIP+フォルダの重複: ZIPフォルダエントリを除去
    /// - 動画+画像の重複: 画像を除去
    /// - 画像の拡張子重複: 優先度の低い拡張子を除去
    fn apply_duplicate_filters(
        &mut self,
        folders: &mut Vec<GridItem>,
        all_media: &mut Vec<(PathBuf, bool, i64, i64)>,
    ) {
        // 1. ZIP + フォルダの重複: 同名フォルダがあれば ZIP エントリをスキップ
        if self.settings.skip_zip_if_folder_exists {
            // 実フォルダの名前 (小文字) を収集
            let real_folder_names: std::collections::HashSet<String> = folders
                .iter()
                .filter_map(|item| {
                    if let GridItem::Folder(p) = item {
                        // .zip 拡張子でないフォルダのみ
                        let is_zip = p
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.eq_ignore_ascii_case("zip"))
                            .unwrap_or(false);
                        if !is_zip {
                            return p
                                .file_name()
                                .and_then(|n| n.to_str())
                                .map(|n| n.to_lowercase());
                        }
                    }
                    None
                })
                .collect();

            // ZIP フォルダエントリを除去（拡張子なしの名前がフォルダ名と一致するもの）
            folders.retain(|item| {
                if let GridItem::Folder(p) = item {
                    let is_zip = p
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.eq_ignore_ascii_case("zip"))
                        .unwrap_or(false);
                    if is_zip {
                        let stem = p
                            .file_stem()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_lowercase();
                        if real_folder_names.contains(&stem) {
                            return false; // スキップ
                        }
                    }
                }
                true
            });
        }

        // 2. 動画 + 画像の重複: 同名の動画があれば画像をスキップし、
        //    画像ファイルを動画のサムネイルソースとして記録する
        if self.settings.skip_image_if_video_exists {
            // 動画ステム → 同名画像パスのマッピングを構築
            let video_stems: std::collections::HashSet<String> = all_media
                .iter()
                .filter(|(_, is_video, _, _)| *is_video)
                .filter_map(|(p, _, _, _)| {
                    p.file_stem()
                        .and_then(|n| n.to_str())
                        .map(|n| n.to_lowercase())
                })
                .collect();

            if !video_stems.is_empty() {
                // 同名画像パスを記録（動画サムネイルに使用）
                for (p, is_video, _, _) in all_media.iter() {
                    if *is_video {
                        continue;
                    }
                    let stem = p
                        .file_stem()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if video_stems.contains(&stem) {
                        self.video_thumb_overrides.insert(stem, p.clone());
                    }
                }

                all_media.retain(|(p, is_video, _, _)| {
                    if *is_video {
                        return true;
                    }
                    let stem = p
                        .file_stem()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    !video_stems.contains(&stem)
                });
            }
        }

        // 3. 同名画像の拡張子重複: 優先度リストに基づいてフィルタ
        if self.settings.skip_duplicate_images {
            let priority = &self.settings.image_ext_priority;

            // ステム → (最優先の拡張子の優先度, インデックス) を記録
            let mut best: std::collections::HashMap<String, (usize, usize)> =
                std::collections::HashMap::new();

            for (i, (p, is_video, _, _)) in all_media.iter().enumerate() {
                if *is_video {
                    continue;
                }
                let stem = p
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let prio = priority
                    .iter()
                    .position(|e| e == &ext)
                    .unwrap_or(usize::MAX);

                match best.get(&stem) {
                    Some(&(existing_prio, _)) if prio >= existing_prio => {
                        // 既存のほうが優先度が高い → 何もしない
                    }
                    _ => {
                        best.insert(stem, (prio, i));
                    }
                }
            }

            // 同名ステムの画像が複数あるか判定
            let mut stem_counts: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for (p, is_video, _, _) in all_media.iter() {
                if *is_video {
                    continue;
                }
                let stem = p
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                *stem_counts.entry(stem).or_insert(0) += 1;
            }

            // 重複があるステムについて、最優先以外を除去
            let keep_indices: std::collections::HashSet<usize> = best
                .iter()
                .filter(|(stem, _)| stem_counts.get(stem.as_str()).copied().unwrap_or(0) > 1)
                .map(|(_, &(_, idx))| idx)
                .collect();

            if !keep_indices.is_empty() {
                let mut i = 0;
                all_media.retain(|(p, is_video, _, _)| {
                    let current_i = i;
                    i += 1;
                    if *is_video {
                        return true;
                    }
                    let stem = p
                        .file_stem()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    // 重複のないステムは常に保持
                    if stem_counts.get(&stem).copied().unwrap_or(0) <= 1 {
                        return true;
                    }
                    // 重複があるステム: 最優先のもののみ保持
                    keep_indices.contains(&current_i)
                });
            }
        }
    }

    /// `search_filter` に基づいて `visible_indices` を再計算する。
    pub(crate) fn rebuild_visible_indices(&mut self) {
        self.visible_indices = match &self.search_filter {
            Some(filter) => (0..self.items.len())
                .filter(|i| filter.contains(i))
                .collect(),
            None => (0..self.items.len()).collect(),
        };
    }

    /// メタデータキーワード検索を実行する。
    /// フォルダ内の全 PNG 画像の tEXt チャンクを読み、
    /// キーワード（大文字小文字無視）にマッチするアイテムのみをフィルタ表示する。
    pub(crate) fn execute_search(&mut self) {
        let query = self.search_query.trim().to_lowercase();
        if query.is_empty() {
            self.search_filter = None;
            self.rebuild_visible_indices();
            return;
        }

        let mut matches = std::collections::HashSet::new();
        for (idx, item) in self.items.iter().enumerate() {
            let path = match item {
                GridItem::Image(p) => p.clone(),
                _ => {
                    // フォルダ・動画・ZIPセパレータは常に表示
                    matches.insert(idx);
                    continue;
                }
            };

            // PNG のみメタデータ検索
            let is_png = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("png"))
                .unwrap_or(false);

            if !is_png {
                // PNG 以外は名前でマッチ
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if name.contains(&query) {
                    matches.insert(idx);
                }
                continue;
            }

            // PNG: tEXt チャンクからメタデータを読んで検索
            let chunks = crate::png_metadata::read_png_text_chunks(&path).unwrap_or_default();
            let mut found = false;
            for (_key, value) in &chunks {
                if value.to_lowercase().contains(&query) {
                    found = true;
                    break;
                }
            }
            // ファイル名でもマッチ
            if !found {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if name.contains(&query) {
                    found = true;
                }
            }
            if found {
                matches.insert(idx);
            }
        }

        self.search_filter = Some(matches);
        self.rebuild_visible_indices();
        self.selected = None;
        self.scroll_offset_y = 0.0;
    }

    /// 指定 idx の回転角度を取得する（キャッシュ + DB）。
    pub(crate) fn get_rotation(&mut self, idx: usize) -> crate::rotation_db::Rotation {
        if let Some(&rot) = self.rotation_cache.get(&idx) {
            return rot;
        }
        let path = match self.items.get(idx) {
            Some(GridItem::Image(p)) => p.clone(),
            Some(GridItem::Video(p)) => p.clone(),
            _ => return crate::rotation_db::Rotation::None,
        };
        let rot = self
            .rotation_db
            .as_ref()
            .and_then(|db| db.get(&path))
            .unwrap_or(crate::rotation_db::Rotation::None);
        self.rotation_cache.insert(idx, rot);
        rot
    }

    /// 指定 idx の画像を時計回りに 90° 回転する。
    pub(crate) fn rotate_image_cw(&mut self, idx: usize) {
        let current = self.get_rotation(idx);
        let new_rot = current.rotate_cw();
        self.apply_rotation(idx, new_rot);
    }

    /// 指定 idx の画像を反時計回りに 90° 回転する。
    pub(crate) fn rotate_image_ccw(&mut self, idx: usize) {
        let current = self.get_rotation(idx);
        let new_rot = current.rotate_ccw();
        self.apply_rotation(idx, new_rot);
    }

    fn apply_rotation(&mut self, idx: usize, rot: crate::rotation_db::Rotation) {
        let path = match self.items.get(idx) {
            Some(GridItem::Image(p)) => p.clone(),
            Some(GridItem::Video(p)) => p.clone(),
            _ => return,
        };
        self.rotation_cache.insert(idx, rot);
        if let Some(ref db) = self.rotation_db {
            let _ = db.set(&path, rot);
        }
    }

    /// 1枚のフルサイズ画像を非同期で読み込み開始する。
    /// 通常画像 / ZIP エントリ の両方に対応。
    /// GIF / APNG はアニメーションフレームを全デコードして FsLoadResult::Animated を送信する。
    fn start_fs_load(&mut self, idx: usize) {
        // (path, zip_entry) を取り出し: 通常画像なら (path, None)、ZIP なら (zip_path, Some(entry))
        let (path, zip_entry) = match self.items.get(idx) {
            Some(GridItem::Image(p)) => (p.clone(), None),
            Some(GridItem::ZipImage { zip_path, entry_name }) => {
                (zip_path.clone(), Some(entry_name.clone()))
            }
            _ => return,
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<FsLoadResult>();
        self.fs_pending.insert(idx, (Arc::clone(&cancel), rx));

        std::thread::spawn(move || {
            if cancel.load(Ordering::Relaxed) { return; }
            let t = std::time::Instant::now();

            // 表示名と拡張子を取得
            let (name, ext) = if let Some(ref entry_name) = zip_entry {
                let base = crate::zip_loader::entry_basename(entry_name).to_string();
                let ext = base.rsplit('.').next().unwrap_or("").to_lowercase();
                (base, ext)
            } else {
                let n = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string();
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                (n, ext)
            };

            // ZIP エントリの場合は先にバイト列を抽出
            let zip_bytes: Option<Vec<u8>> = if let Some(ref entry_name) = zip_entry {
                match crate::zip_loader::read_entry_bytes(&path, entry_name) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        crate::logger::log(format!(
                            "  fs zip read FAIL: {e}  {name}"
                        ));
                        return;
                    }
                }
            } else {
                None
            };

            // GIF: アニメーション試行 (通常パスのみ, ZIP は未対応)
            if ext == "gif" && zip_bytes.is_none() {
                if let Some(frames) = decode_gif_frames(&path) {
                    crate::logger::log(format!(
                        "  fs load anim-gif: {:.0}ms  idx={idx}  {name}  {} frames",
                        t.elapsed().as_secs_f64() * 1000.0,
                        frames.len()
                    ));
                    let _ = tx.send(FsLoadResult::Animated(frames));
                    return;
                }
            }

            // PNG: APNG アニメーション試行 (通常パスのみ, ZIP は未対応)
            if ext == "png" && zip_bytes.is_none() {
                if let Some(frames) = decode_apng_frames(&path) {
                    crate::logger::log(format!(
                        "  fs load anim-png: {:.0}ms  idx={idx}  {name}  {} frames",
                        t.elapsed().as_secs_f64() * 1000.0,
                        frames.len()
                    ));
                    let _ = tx.send(FsLoadResult::Animated(frames));
                    return;
                }
            }

            // 静止画フォールバック
            // image クレート → (失敗時) WIC の順で試す
            // ZIP エントリの場合は WIC がファイルパスを必要とするため image クレートのみ
            let open_result = if let Some(bytes) = zip_bytes {
                image::load_from_memory(&bytes)
            } else {
                match image::open(&path) {
                    Ok(img) => Ok(img),
                    Err(e) => match crate::wic_decoder::decode_to_dynamic_image(&path) {
                        Some(img) => Ok(img),
                        None => Err(e),
                    },
                }
            };
            match open_result {
                Ok(img) => {
                    // EXIF Orientation 自動回転 (ZIP 以外)
                    let img = if zip_entry.is_none() {
                        crate::thumb_loader::apply_exif_orientation(img, &path)
                    } else {
                        img
                    };
                    let rgba = img.to_rgba8();
                    let (w, h) = (rgba.width(), rgba.height());
                    let size = [w as usize, h as usize];
                    let ci = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                    drop(rgba);
                    crate::logger::log(format!(
                        "  fs load: {:.0}ms  idx={idx}  {name}  {w}x{h}",
                        t.elapsed().as_secs_f64() * 1000.0
                    ));
                    let _ = tx.send(FsLoadResult::Static(ci));
                }
                Err(e) => {
                    crate::logger::log(format!("  fs load FAIL: {e}  {name}"));
                    // UI が「読込中...」のまま固まらないよう、失敗を明示的に通知する
                    let _ = tx.send(FsLoadResult::Failed);
                }
            }
        });
    }

    /// 先読みウィンドウを更新する。
    /// settings の prefetch_back / prefetch_forward に従って先読みを開始し、
    /// ウィンドウ外のキャッシュ・読み込みを破棄する。
    fn update_prefetch_window(&mut self, current_idx: usize) {
        let image_indices = Self::collect_image_indices(&self.items);
        let Some(pos) = image_indices.iter().position(|&i| i == current_idx) else { return; };
        let n = image_indices.len();

        let pf_back    = self.settings.prefetch_back;
        let pf_forward = self.settings.prefetch_forward;
        // KEEP はそれぞれ +1 だけ広く保持してテクスチャ破棄を遅延させる
        let keep_back    = pf_back + 1;
        let keep_forward = pf_forward + 1;

        let keep_set: std::collections::HashSet<usize> =
            (pos.saturating_sub(keep_back)..=((pos + keep_forward).min(n - 1)))
                .map(|p| image_indices[p])
                .collect();

        // 前方優先（+1, +2, … , -1, -2, …）の順で起動する
        let forward_targets = (1..=pf_forward)
            .map(|d| pos + d)
            .filter(|&p| p < n)
            .map(|p| image_indices[p]);
        let back_targets = (1..=pf_back)
            .map(|d| pos.wrapping_sub(d))
            .filter(|&p| p < n) // wrapping_sub で usize がラップした場合も除外
            .map(|p| image_indices[p]);
        let prefetch_targets: Vec<usize> = forward_targets.chain(back_targets).collect();

        // KEEP 範囲外のテクスチャを破棄（VRAM 節約）
        self.fs_cache.retain(|k, _| keep_set.contains(k));

        // KEEP 範囲外の読み込みをキャンセル・破棄
        let to_cancel: Vec<usize> = self.fs_pending.keys()
            .filter(|k| !keep_set.contains(k))
            .cloned()
            .collect();
        for k in to_cancel {
            if let Some((cancel, _)) = self.fs_pending.remove(&k) {
                cancel.store(true, Ordering::Relaxed);
            }
        }

        // まだキャッシュにも pending にもない先読み対象を読み込み開始
        for idx in prefetch_targets {
            if !self.fs_cache.contains_key(&idx) && !self.fs_pending.contains_key(&idx) {
                crate::logger::log(format!("  prefetch start idx={idx}"));
                self.start_fs_load(idx);
            }
        }
    }

    /// items の中の画像アイテム (通常 + ZIP 内) の item_idx 一覧を返す（先読みウィンドウ用）
    fn collect_image_indices(items: &[GridItem]) -> Vec<usize> {
        items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                matches!(item, GridItem::Image(_) | GridItem::ZipImage { .. }).then_some(i)
            })
            .collect()
    }


    /// フルスクリーン表示を終了し、先読みキャッシュを全クリアする。
    pub(crate) fn close_fullscreen(&mut self) {
        self.fullscreen_idx = None;
        self.slideshow_playing = false;
        for (cancel, _) in self.fs_pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.fs_pending.clear();
        self.fs_cache.clear();
    }

    /// `self.selected` に対応するアイテムが画像の場合、パスを last_selected_image_path に保存する。
    /// (フォルダ移動後もサムネイル画質ダイアログで使えるよう、セッション内で保持)
    pub(crate) fn update_last_selected_image(&mut self) {
        if let Some(idx) = self.selected {
            if let Some(GridItem::Image(p)) = self.items.get(idx) {
                self.last_selected_image_path = Some(p.clone());
            }
        }
    }

    /// pending の読み込みをポーリングし、完了したものをキャッシュに取り込む。
    fn poll_prefetch(&mut self, ctx: &egui::Context) {
        let mut completed: Vec<(usize, FsLoadResult)> = Vec::new();
        for (&key, (_, rx)) in &self.fs_pending {
            if let Ok(result) = rx.try_recv() {
                completed.push((key, result));
            }
        }
        let repaint = !completed.is_empty();
        for (key, result) in completed {
            self.fs_pending.remove(&key);
            let entry = match result {
                FsLoadResult::Static(ci) => {
                    let handle = ctx.load_texture(
                        format!("fs_{key}"),
                        ci,
                        egui::TextureOptions::LINEAR,
                    );
                    FsCacheEntry::Static(handle)
                }
                FsLoadResult::Animated(frames) => {
                    let textures: Vec<(egui::TextureHandle, f64)> = frames
                        .into_iter()
                        .enumerate()
                        .map(|(fi, (ci, delay))| {
                            let handle = ctx.load_texture(
                                format!("fs_{key}_f{fi}"),
                                ci,
                                egui::TextureOptions::LINEAR,
                            );
                            (handle, delay)
                        })
                        .collect();
                    let now = ctx.input(|i| i.time);
                    let first_delay = textures.first().map(|(_, d)| *d).unwrap_or(0.1);
                    FsCacheEntry::Animated {
                        frames: textures,
                        current_frame: 0,
                        next_frame_at: now + first_delay,
                    }
                }
                FsLoadResult::Failed => FsCacheEntry::Failed,
            };
            self.fs_cache.insert(key, entry);
        }
        if repaint {
            ctx.request_repaint();
        }
    }

    // -------------------------------------------------------------------
    // サムネイル画質設定ダイアログ (A/B 比較)
    // -------------------------------------------------------------------
    pub(crate) fn open_thumb_quality_dialog(&mut self, ctx: &egui::Context) {
        // 既存状態をリセット
        self.tq.sample = None;
        self.tq.sample_path = None;
        self.tq.sample_original_size = 0;
        self.tq.a_texture = None;
        self.tq.b_texture = None;
        self.tq.a_bytes = 0;
        self.tq.b_bytes = 0;

        // 最後に選択した画像を取得
        let Some(path) = self.last_selected_image_path.clone() else {
            // None のままダイアログを開く (メッセージだけ出る)
            self.tq.show = true;
            return;
        };

        // サンプル画像をデコード
        let img = match image::open(&path) {
            Ok(i) => i,
            Err(_) => {
                self.tq.show = true;
                return;
            }
        };
        let orig_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        self.tq.sample = Some(img);
        self.tq.sample_path = Some(path);
        self.tq.sample_original_size = orig_size;

        // 現在の設定で A を初期化、B はちょっと違う組み合わせ
        self.tq.a_size = self.settings.thumb_px;
        self.tq.a_quality = self.settings.thumb_quality;
        self.tq.b_size = self.settings.thumb_px;
        self.tq.b_quality = (self.settings.thumb_quality as u32 + 10).min(95) as u8;

        self.reencode_tq_panel(ctx, true);
        self.reencode_tq_panel(ctx, false);
        self.tq.show = true;
    }

    pub(crate) fn reencode_tq_panel(&mut self, ctx: &egui::Context, is_a: bool) {
        let Some(img) = self.tq.sample.as_ref() else { return };
        let (size, quality) = if is_a {
            (self.tq.a_size, self.tq.a_quality)
        } else {
            (self.tq.b_size, self.tq.b_quality)
        };
        let (bytes, tex) =
            match crate::catalog::encode_thumb_webp(img, size, quality as f32) {
                Some((data, _w, _h)) => {
                    let byte_len = data.len();
                    let color_image = crate::catalog::decode_thumb_to_color_image(&data);
                    let tex = color_image.map(|ci| {
                        ctx.load_texture(
                            format!("tq_preview_{}", if is_a { "a" } else { "b" }),
                            ci,
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    (byte_len, tex)
                }
                None => (0, None),
            };
        if is_a {
            self.tq.a_bytes = bytes;
            self.tq.a_texture = tex;
        } else {
            self.tq.b_bytes = bytes;
            self.tq.b_texture = tex;
        }
    }

    pub(crate) fn close_thumb_quality_dialog(&mut self) {
        self.tq.show = false;
        self.tq.sample = None;
        self.tq.sample_path = None;
        self.tq.a_texture = None;
        self.tq.b_texture = None;
        self.tq.fullscreen = false;
    }

    // -------------------------------------------------------------------
    // キャッシュ作成（バックグラウンドで選択フォルダ以下を再帰処理）
    // -------------------------------------------------------------------
    pub(crate) fn start_cache_creation(&mut self) {
        // 選択されたお気に入りを集める
        let targets: Vec<PathBuf> = self
            .settings
            .favorites
            .iter()
            .zip(self.cc.checked.iter())
            .filter_map(|(f, &c)| if c { Some(f.path.clone()) } else { None })
            .collect();

        if targets.is_empty() {
            return;
        }

        // 状態リセット
        self.cc.running = true;
        self.cc.counting.store(true, Ordering::Relaxed);
        self.cc.total.store(0, Ordering::Relaxed);
        self.cc.done.store(0, Ordering::Relaxed);
        self.cc.finished.store(false, Ordering::Relaxed);
        self.cc.result = None;
        *self.cc.current.lock().unwrap() = String::new();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cc.cancel = Arc::clone(&cancel);

        // 初期キャッシュ容量を取得（ベースライン）
        let cache_dir = crate::catalog::default_cache_dir();
        let (_, baseline) = crate::catalog::cache_stats(&cache_dir);
        self.cc.cache_size
            .store(baseline, Ordering::Relaxed);

        // atomic クローン
        let counting = Arc::clone(&self.cc.counting);
        let total = Arc::clone(&self.cc.total);
        let done = Arc::clone(&self.cc.done);
        let size_atomic = Arc::clone(&self.cc.cache_size);
        let finished = Arc::clone(&self.cc.finished);
        let current = Arc::clone(&self.cc.current);
        let thumb_px = self.settings.thumb_px;
        let thumb_quality = self.settings.thumb_quality;
        let threads = self.settings.parallelism.thread_count();

        std::thread::spawn(move || {
            // Pass 1: カウント
            let mut all_folders: Vec<PathBuf> = Vec::new();
            for t in &targets {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                walk_dirs_recursive(t, &mut all_folders, &cancel);
            }
            total.store(all_folders.len(), Ordering::Relaxed);
            counting.store(false, Ordering::Relaxed);

            if cancel.load(Ordering::Relaxed) {
                finished.store(true, Ordering::Relaxed);
                return;
            }

            // 処理用 rayon プール
            let pool = match rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
            {
                Ok(p) => p,
                Err(_) => {
                    finished.store(true, Ordering::Relaxed);
                    return;
                }
            };

            // Pass 2: フォルダを順次処理、内部画像は並列デコード
            for folder in &all_folders {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }

                *current.lock().unwrap() = folder.to_string_lossy().to_string();

                // 画像列挙（単一フォルダ、再帰なし）
                let mut images: Vec<(PathBuf, i64, i64)> = Vec::new();
                if let Ok(entries) = std::fs::read_dir(folder) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if !p.is_file() || is_apple_double(&p) {
                            continue;
                        }
                        let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
                            continue;
                        };
                        let ext_lower = ext.to_ascii_lowercase();
                        if !SUPPORTED_EXTENSIONS.contains(&ext_lower.as_str()) {
                            continue;
                        }
                        let meta = entry.metadata().ok();
                        let mtime = meta
                            .as_ref()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map_or(0, |d| d.as_secs() as i64);
                        let file_size = meta.map_or(0, |m| m.len() as i64);
                        images.push((p, mtime, file_size));
                    }
                }

                if images.is_empty() {
                    done.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                // カタログを開く（1フォルダ1DB）
                let Ok(catalog) = crate::catalog::CatalogDb::open(&cache_dir, folder) else {
                    done.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let cache_map = catalog.load_all().unwrap_or_default();

                // 並列でデコード + 保存
                pool.install(|| {
                    use rayon::prelude::*;
                    images.par_iter().for_each(|(path, mtime, file_size)| {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let filename = match path.file_name().and_then(|n| n.to_str()) {
                            Some(n) => n,
                            None => return,
                        };
                        // 既存キャッシュチェック
                        if let Some(entry) = cache_map.get(filename) {
                            if entry.mtime == *mtime && entry.file_size == *file_size {
                                return;
                            }
                        }
                        if let Some(bytes) = build_and_save_one(
                            path,
                            &catalog,
                            *mtime,
                            *file_size,
                            thumb_px,
                            thumb_quality,
                        ) {
                            size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                        }
                    });
                });

                done.fetch_add(1, Ordering::Relaxed);
            }

            finished.store(true, Ordering::Relaxed);
        });
    }








}

// -----------------------------------------------------------------------
// eframe::App 実装
// -----------------------------------------------------------------------

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 初回フレームで前回フォルダを復元
        // ZIP ファイルや、削除済み・取り外し済みのパスでもクラッシュしないよう
        // resolve_openable_path で最も近い既存ディレクトリに解決する。
        if !self.initialized {
            self.initialized = true;
            if let Some(folder) = self.settings.last_folder.clone() {
                if let Some(resolved) = crate::folder_tree::resolve_openable_path(&folder) {
                    self.load_folder(resolved);
                }
            }
        }

        // ウィンドウ位置を記録（最小化・最大化中は更新しない）
        // outer_rect が None の場合は inner_rect で代用する（egui バックエンドによって異なる）
        {
            let (outer_rect, inner_rect, pixels_per_point, minimized, maximized) =
                ctx.input(|i| {
                    let vp = i.viewport();
                    (
                        vp.outer_rect,
                        vp.inner_rect,
                        i.pixels_per_point,
                        vp.minimized.unwrap_or(false),
                        vp.maximized.unwrap_or(false),
                    )
                });

            // outer_rect が None のフレームをログ（初回のみ）
            if outer_rect.is_none() && self.last_outer_rect.is_none() {
                crate::logger::log(format!(
                    "[viewport] outer_rect=None  inner_rect={:?}  pixels_per_point={pixels_per_point:.2}",
                    inner_rect.map(|r| format!("pos=({:.0},{:.0}) size={:.0}x{:.0}",
                        r.min.x, r.min.y, r.width(), r.height()))
                ));
            }

            // outer_rect 優先、なければ inner_rect を使用
            let best_rect = outer_rect.or(inner_rect);

            // ppp は最小化・最大化に関係なく常に更新する
            self.last_pixels_per_point = pixels_per_point;

            if !minimized && !maximized {
                if let Some(rect) = best_rect {
                    let changed = self.last_outer_rect
                        .map(|r| (r.min - rect.min).length() > 1.0
                                 || (r.size() - rect.size()).length() > 1.0)
                        .unwrap_or(true);
                    if changed {
                        crate::logger::log(format!(
                            "[viewport] rect updated: pos=({:.0},{:.0}) size={:.0}x{:.0}  \
                             outer={:?}  inner={:?}  ppp={pixels_per_point:.2}",
                            rect.min.x, rect.min.y, rect.width(), rect.height(),
                            outer_rect.map(|_| "Some"),
                            inner_rect.map(|_| "Some"),
                        ));
                        self.last_outer_rect = Some(rect);
                    }
                }
            }
        }

        // 毎フレームリセット: 選択セルが描画された時に再設定される
        self.selected_cell_rect = None;

        self.poll_thumbnails(ctx);
        self.update_keep_range_and_requests();
        self.poll_prefetch(ctx);

        // タイトルバーに現在のフォルダパスを表示する。
        // フォルダ未選択時や読み込み途中はアプリ名のみ。
        let title = match self.current_folder.as_ref() {
            Some(p) => format!("{} - mimageviewer", p.display()),
            None => "mimageviewer".to_string(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        // スクロールは egui に触れる前に処理（イベントを消費）
        self.process_scroll(ctx);

        // ── Ctrl+C / Ctrl+X / Ctrl+V ショートカット ─────────────────
        let main_focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if main_focused && !self.any_dialog_open() && !self.address_has_focus
            && !self.search_has_focus && self.fullscreen_idx.is_none()
        {
            // Ctrl+C/X: egui は Copy/Cut イベントに変換する
            let (ctrl_c, ctrl_x) = ctx.input(|i| {
                let mut c = false;
                let mut x = false;
                for event in &i.events {
                    match event {
                        egui::Event::Copy => c = true,
                        egui::Event::Cut => x = true,
                        _ => {}
                    }
                }
                (c, x)
            });
            // Ctrl+V: egui/winit はクリップボードにテキストがない場合
            // Paste イベントも Key::V イベントも発生しない。
            // Windows API (GetAsyncKeyState) で直接キー状態を確認する。
            let ctrl_v = {
                #[cfg(windows)]
                {
                    // VK_CONTROL=0x11, VK_V=0x56
                    // GetAsyncKeyState の最上位ビットが 1 ならキーが押されている
                    let ctrl = unsafe {
                        windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x11)
                    };
                    let v = unsafe {
                        windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x56)
                    };
                    // 両方が押されている && 前フレームでは押されていなかった
                    let pressed = (ctrl & (0x8000u16 as i16)) != 0
                        && (v & (0x8000u16 as i16)) != 0
                        && (v & 1) != 0; // 1 = 前回チェック以降に押された
                    pressed
                }
                #[cfg(not(windows))]
                false
            };

            if ctrl_c || ctrl_x {
                let paths = if !self.checked.is_empty() {
                    self.collect_checked_paths()
                } else if let Some(idx) = self.selected {
                    match self.items.get(idx) {
                        Some(GridItem::Image(p)) | Some(GridItem::Video(p)) => vec![p.clone()],
                        _ => vec![],
                    }
                } else {
                    vec![]
                };
                if !paths.is_empty() {
                    if ctrl_x {
                        crate::ui_dialogs::context_menu::cut_files_to_clipboard(&paths);
                    } else {
                        crate::ui_dialogs::context_menu::copy_files_to_clipboard(&paths);
                    }
                }
            }

            if ctrl_v {
                if let Some(ref folder) = self.current_folder.clone() {
                    crate::ui_dialogs::context_menu::paste_files_from_clipboard(folder);
                    self.pending_reload = true;
                }
            }
        }

        let keyboard_nav = self.handle_keyboard(ctx);

        // ── フルスクリーンビューポート ──────────────────────────────────
        // 非アクティブ時も非表示でビューポートを維持（次回表示のちらつき防止）
        self.keep_fullscreen_viewport_alive(ctx);
        self.render_fullscreen_viewport(ctx);

        // ── メニューバー ─────────────────────────────────────────────
        let (fav_nav, _) = self.render_menubar(ctx);

        // ── 進捗バー (左下フローティングオーバーレイ) ────────────────
        self.render_progress_overlay(ctx);

        // ── ダイアログ群 ─────────────────────────────────────────────
        self.show_favorites_editor_dialog(ctx);
        self.show_fav_add_dialog_window(ctx);
        let open_folder_nav = self.show_open_folder_dialog_window(ctx);
        self.show_cache_manager_dialog(ctx);
        self.show_cache_creator_dialog(ctx);
        self.show_thumb_quality_dialog_window(ctx);
        self.show_thumb_quality_fullscreen_overlay(ctx);
        self.show_preferences_dialog(ctx);
        self.show_toolbar_settings_dialog(ctx);
        self.show_cache_policy_dialog(ctx);
        self.show_stats_dialog_window(ctx);
        self.show_exif_settings_dialog(ctx);
        self.show_slideshow_settings_dialog(ctx);
        self.show_rotation_reset_confirm_dialog(ctx);
        self.show_duplicate_settings_dialog(ctx);
        let context_nav = self.show_context_menu(ctx);
        self.show_delete_confirm_dialog(ctx);

        // ── ツールバー ───────────────────────────────────────────────
        let toolbar_fav_nav = self.render_toolbar(ctx);

        // ── アドレスバー ─────────────────────────────────────────────
        let address_nav = self.render_address_bar(ctx);

        // ── Ctrl+F: 検索バー表示 ─────────────────────────────────────
        if !self.address_has_focus && self.fullscreen_idx.is_none() && !self.any_dialog_open() {
            let ctrl_f = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::F));
            if ctrl_f {
                self.show_search_bar = true;
                self.search_focus_request = true;
            }
        }

        // ── Ctrl+O: フォルダを開く ───────────────────────────────────
        if self.fullscreen_idx.is_none() && !self.any_dialog_open() {
            let ctrl_o = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::O));
            if ctrl_o {
                self.open_folder_input = self
                    .current_folder
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.show_open_folder_dialog = true;
            }
        }

        // ── 検索バー ─────────────────────────────────────────────────
        self.render_search_bar(ctx);

        // ── サムネイルグリッド ────────────────────────────────────────
        let grid_nav = self.render_grid(ctx);

        // ── 選択情報オーバーレイ ─────────────────────────────────────
        self.render_selection_info(ctx);

        // ── DEL キー ──────────────────────────────────────────────────
        self.handle_delete_key(ctx);

        // ── ペースト後のフォルダ再読み込み ────────────────────────
        if self.pending_reload {
            self.pending_reload = false;
            if let Some(folder) = self.current_folder.clone() {
                // 少し遅延してからリロード（ペースト処理の完了を待つ）
                ctx.request_repaint();
                self.load_folder(folder);
            }
        }

        // ── ナビゲーション集約 ───────────────────────────────────────
        let navigate = fav_nav
            .or(toolbar_fav_nav)
            .or(keyboard_nav)
            .or(address_nav)
            .or(open_folder_nav)
            .or(context_nav)
            .or(grid_nav);
        if let Some(p) = navigate {
            self.load_folder(p);
        }

        // Pending なサムネイルがある間は毎フレーム再描画をリクエストする。
        // バックグラウンドスレッドがチャネルに送信しても egui は自動では
        // 起きないため、ここで継続的に repaint を要求しておく必要がある。
        if self.thumbnails.iter().any(|t| matches!(t, ThumbnailState::Pending)) {
            ctx.request_repaint();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // 終了時にウィンドウ位置・サイズを保存
        if let Some(rect) = self.last_outer_rect {
            self.settings.window_pos  = Some([rect.min.x, rect.min.y]);
            self.settings.window_size = Some([rect.width(), rect.height()]);
        }
        self.settings.save();
    }
}

// -----------------------------------------------------------------------
// セル描画
// -----------------------------------------------------------------------

/// GridItem から LoadRequest を構築する。画像 / ZIP 内画像以外は None を返す。
fn make_load_request(
    item: &GridItem,
    idx: usize,
    mtime: i64,
    file_size: i64,
    skip_cache: bool,
) -> Option<LoadRequest> {
    match item {
        GridItem::Image(p) => Some(LoadRequest {
            idx, path: p.clone(), mtime, file_size,
            skip_cache, zip_entry: None,
        }),
        GridItem::ZipImage { zip_path, entry_name } => Some(LoadRequest {
            idx, path: zip_path.clone(), mtime, file_size,
            skip_cache, zip_entry: Some(entry_name.clone()),
        }),
        _ => None,
    }
}

/// サムネイルテクスチャをアスペクト保持で中央配置して描画する（回転対応）。
fn draw_thumb_texture(
    painter: &egui::Painter,
    inner: egui::Rect,
    tex: &egui::TextureHandle,
    rotation: crate::rotation_db::Rotation,
) {
    let tex_size = tex.size_vec2();
    // 90°/270° 回転時は幅と高さが入れ替わる
    let display_size = match rotation {
        crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
            egui::vec2(tex_size.y, tex_size.x)
        }
        _ => tex_size,
    };
    let scale = (inner.width() / display_size.x).min(inner.height() / display_size.y);
    let img_rect = egui::Rect::from_center_size(inner.center(), display_size * scale);

    if rotation.is_none() {
        painter.image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        // 回転したテクスチャを Mesh で描画
        draw_rotated_image(painter, tex.id(), img_rect, rotation);
    }
}

/// 回転した画像を Mesh で描画する。
pub(crate) fn draw_rotated_image(
    painter: &egui::Painter,
    texture_id: egui::TextureId,
    rect: egui::Rect,
    rotation: crate::rotation_db::Rotation,
) {
    // UV 座標を回転に合わせて変換
    // 頂点順: 左上, 右上, 右下, 左下 (画面座標)
    // UV は回転に応じて異なる頂点に割り当てる
    let uvs = match rotation {
        crate::rotation_db::Rotation::None => [
            egui::pos2(0.0, 0.0), // 左上
            egui::pos2(1.0, 0.0), // 右上
            egui::pos2(1.0, 1.0), // 右下
            egui::pos2(0.0, 1.0), // 左下
        ],
        crate::rotation_db::Rotation::Cw90 => [
            egui::pos2(0.0, 1.0),
            egui::pos2(0.0, 0.0),
            egui::pos2(1.0, 0.0),
            egui::pos2(1.0, 1.0),
        ],
        crate::rotation_db::Rotation::Cw180 => [
            egui::pos2(1.0, 1.0),
            egui::pos2(0.0, 1.0),
            egui::pos2(0.0, 0.0),
            egui::pos2(1.0, 0.0),
        ],
        crate::rotation_db::Rotation::Cw270 => [
            egui::pos2(1.0, 0.0),
            egui::pos2(1.0, 1.0),
            egui::pos2(0.0, 1.0),
            egui::pos2(0.0, 0.0),
        ],
    };

    let positions = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];

    let mut mesh = egui::Mesh::with_texture(texture_id);
    for i in 0..4 {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: positions[i],
            uv: uvs[i],
            color: egui::Color32::WHITE,
        });
    }
    mesh.indices = vec![0, 1, 2, 0, 2, 3];
    painter.add(egui::Shape::mesh(mesh));
}

/// 画像系アイテム (Image / ZipImage) のサムネイル状態に応じた描画。
fn draw_thumb(
    painter: &egui::Painter,
    inner: egui::Rect,
    thumb: &ThumbnailState,
    rotation: crate::rotation_db::Rotation,
) {
    match thumb {
        ThumbnailState::Loaded { tex, .. } => {
            draw_thumb_texture(painter, inner, tex, rotation);
        }
        ThumbnailState::Pending | ThumbnailState::Evicted => {
            painter.rect_filled(inner, 2.0, egui::Color32::from_gray(220));
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "読込中",
                egui::FontId::proportional(12.0),
                egui::Color32::from_gray(140),
            );
        }
        ThumbnailState::Failed => {
            painter.rect_filled(inner, 2.0, egui::Color32::from_rgb(255, 220, 220));
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "読込失敗",
                egui::FontId::proportional(12.0),
                egui::Color32::DARK_RED,
            );
        }
    }
}

pub(crate) fn draw_cell(
    ui: &egui::Ui,
    rect: egui::Rect,
    is_selected: bool,
    is_checked: bool,
    item: &GridItem,
    thumb: &ThumbnailState,
    rotation: crate::rotation_db::Rotation,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter();
    let padding = 4.0;
    let inner = rect.shrink(padding);

    let bg = if is_selected {
        egui::Color32::from_rgb(180, 210, 255)
    } else {
        egui::Color32::WHITE
    };
    painter.rect_filled(rect, 2.0, bg);

    match item {
        GridItem::Folder(path) => {
            painter.text(
                inner.center() - egui::vec2(0.0, 14.0),
                egui::Align2::CENTER_CENTER,
                "📁",
                egui::FontId::proportional(42.0),
                egui::Color32::from_rgb(220, 170, 30),
            );
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            painter.text(
                egui::pos2(inner.center().x, inner.max.y - 4.0),
                egui::Align2::CENTER_BOTTOM,
                truncate_name(name, 18),
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(30),
            );
        }
        GridItem::Image(_) => {
            draw_thumb(painter, inner, thumb, rotation);
        }
        GridItem::Video(path) => {
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    draw_thumb_texture(painter, inner, tex, rotation);
                }
                ThumbnailState::Pending | ThumbnailState::Evicted => {
                    painter.rect_filled(inner, 2.0, egui::Color32::from_gray(40));
                    painter.text(
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        "動画",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_gray(160),
                    );
                }
                ThumbnailState::Failed => {
                    painter.rect_filled(inner, 2.0, egui::Color32::from_gray(40));
                }
            }
            // 再生ボタンオーバーレイ（常時表示）
            let r = (inner.width().min(inner.height()) * 0.18).max(10.0);
            draw_play_icon(painter, inner.center(), r);
            // ファイル名
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            painter.text(
                egui::pos2(inner.center().x, inner.max.y - 4.0),
                egui::Align2::CENTER_BOTTOM,
                truncate_name(name, 18),
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(30),
            );
        }
        GridItem::ZipImage { .. } => {
            draw_thumb(painter, inner, thumb, rotation);
        }
        GridItem::ZipSeparator { dir_display } => {
            // 作品境界のセパレータ: 1 セル全体に目立つ背景 + フォルダ名
            painter.rect_filled(
                inner,
                6.0,
                egui::Color32::from_rgb(235, 242, 252),
            );
            painter.rect_stroke(
                inner,
                6.0,
                egui::Stroke::new(2.0, egui::Color32::from_rgb(120, 160, 220)),
                egui::StrokeKind::Middle,
            );
            // フォルダ名を大きめの太字で中央に
            let font_size = (inner.height() * 0.14).clamp(14.0, 36.0);
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                truncate_name(dir_display, 24),
                egui::FontId::proportional(font_size),
                egui::Color32::from_rgb(40, 70, 140),
            );
            // 下部にフォルダアイコン的な記号
            let small = (inner.height() * 0.08).clamp(9.0, 16.0);
            painter.text(
                egui::pos2(inner.center().x, inner.max.y - 6.0),
                egui::Align2::CENTER_BOTTOM,
                "📁  作品の区切り",
                egui::FontId::proportional(small),
                egui::Color32::from_gray(100),
            );
        }
    }

    let border = if is_selected {
        egui::Stroke::new(2.0, egui::Color32::from_rgb(60, 120, 220))
    } else {
        egui::Stroke::new(1.0, egui::Color32::from_gray(200))
    };
    painter.rect_stroke(rect, 2.0, border, egui::StrokeKind::Middle);

    // チェックマークオーバーレイ
    if is_checked {
        let check_r = 12.0;
        let check_center = egui::pos2(rect.max.x - check_r - 4.0, rect.min.y + check_r + 4.0);
        painter.circle_filled(
            check_center,
            check_r,
            egui::Color32::from_rgb(40, 140, 40),
        );
        // チェックマーク (✓)
        let s = check_r * 0.55;
        let stroke = egui::Stroke::new(2.5, egui::Color32::WHITE);
        painter.line_segment(
            [
                egui::pos2(check_center.x - s * 0.6, check_center.y),
                egui::pos2(check_center.x - s * 0.1, check_center.y + s * 0.5),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(check_center.x - s * 0.1, check_center.y + s * 0.5),
                egui::pos2(check_center.x + s * 0.7, check_center.y - s * 0.5),
            ],
            stroke,
        );
    }
}

/// サムネイル画質プレビュー用: 実グリッドと同じ `cell_w × cell_h` のセルを描画する。
/// 白背景 + 4px パディング、画像はアスペクト保持で中央配置（draw_cell と同じ方式）。
/// クリック可能で、クリック時は Response.clicked() が true になる。
pub(crate) fn tq_draw_preview(
    ui: &mut egui::Ui,
    tex: &Option<egui::TextureHandle>,
    cell_w: f32,
    cell_h: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(cell_w, cell_h),
        egui::Sense::click(),
    );
    let painter = ui.painter();
    // 白背景（選択状態ではないグリッドセルと同じ）
    painter.rect_filled(rect, 2.0, egui::Color32::WHITE);

    let padding = 4.0;
    let inner = rect.shrink(padding);

    match tex {
        Some(t) => {
            let tex_size = t.size_vec2();
            let scale = (inner.width() / tex_size.x).min(inner.height() / tex_size.y);
            let img_size = tex_size * scale;
            let img_rect = egui::Rect::from_center_size(inner.center(), img_size);
            painter.image(
                t.id(),
                img_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "エンコード失敗",
                egui::FontId::proportional(14.0),
                egui::Color32::from_gray(120),
            );
        }
    }

    // ホバー時にカーソル変更 + 縁を青くしてクリック可能さを示す
    if response.hovered() {
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 150, 220)),
            egui::StrokeKind::Outside,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    response
}
