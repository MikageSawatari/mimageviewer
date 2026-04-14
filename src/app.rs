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

// キャッシュキー定数は thumb_loader.rs に定義 (ベンチマーク bin からも参照するため)
pub(crate) use crate::thumb_loader::{CACHE_KEY_ZIP, CACHE_KEY_PDF, CACHE_KEY_FOLDER};

/// パスからファイル名のステム部分を小文字で取得するヘルパー。
fn stem_lower(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase()
}
use crate::fs_animation::{decode_apng_frames, decode_gif_frames, FsCacheEntry, FsLoadResult};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::thumb_loader::{
    build_and_save_one, compute_display_px, encode_and_save, process_load_request, CacheDecision, LoadRequest,
    ThumbMsg,
};
use crate::ui_helpers::{
    draw_folder_badge, draw_pdf_badge, draw_play_icon, draw_zip_badge, natural_sort_key,
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
    /// 通常画像 (Image, ZipImage, PdfPage) 用
    pub(crate) reload_queue: Option<Arc<Mutex<Vec<LoadRequest>>>>,
    /// 重い I/O (ZipFile, PdfFile, Folder) 用の専用キュー。
    /// 専用 I/O ワーカー (2本) が priority 順に取り出す。
    pub(crate) heavy_io_queue: Option<Arc<Mutex<Vec<LoadRequest>>>>,
    /// ロード要求を送ったがまだ応答が来ていない idx 集合（重複要求防止）。
    /// 値は `true` ならアイドル時アップグレード要求、`false` なら通常の読み込み要求。
    pub(crate) requested: std::collections::HashMap<usize, bool>,
    /// 現在の keep range [start, end)。update_keep_range で毎フレーム更新
    pub(crate) keep_range: (usize, usize),
    /// ワーカー共有用: keep_range の start/end をアトミックに公開。
    /// ワーカーは pick 後にこの範囲を確認し、範囲外のリクエストをスキップする。
    pub(crate) keep_start_shared: Arc<AtomicUsize>,
    pub(crate) keep_end_shared: Arc<AtomicUsize>,
    /// poll_thumbnails で 1 フレームのテクスチャ生成上限を超えた分を次フレームに持ち越す
    texture_backlog: Vec<crate::thumb_loader::ThumbMsg>,

    /// Ctrl+↑↓ のバックグラウンドフォルダナビゲーション結果待ち。
    /// navigate_folder_with_skip をワーカースレッドで実行し、UIスレッドをブロックしない。
    /// (キャンセルトークン, 結果レシーバー)
    folder_nav_pending: Option<(Arc<AtomicBool>, mpsc::Receiver<Option<PathBuf>>)>,

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

    // ── 統合環境設定ダイアログ ─────────────────────────────────────
    pub(crate) show_preferences: bool,
    /// 統合環境設定の一時編集状態
    pub(crate) pref_state: Option<crate::ui_dialogs::preferences::PreferencesState>,

    // ── 複数選択 ──────────────────────────────────────────────────
    /// チェック済みアイテムの集合 (スペースキーで追加/削除)
    pub(crate) checked: std::collections::HashSet<usize>,

    // ── 右クリックコンテキストメニュー ─────────────────────────
    /// コンテキストメニューの対象アイテムインデックス
    pub(crate) context_menu_idx: Option<usize>,
    /// コンテキストメニューの表示座標 (右クリック時に記録)
    pub(crate) context_menu_pos: egui::Pos2,

    // ── フルスクリーン右クリックコンテキストメニュー ─────────
    /// 右クリック長押し検出用: 押下開始時刻と座標
    pub(crate) fs_secondary_press_start: Option<(std::time::Instant, egui::Pos2)>,
    /// フルスクリーン用コンテキストメニューの対象アイテムインデックス
    pub(crate) fs_context_menu_idx: Option<usize>,
    /// フルスクリーン用コンテキストメニューの表示座標
    pub(crate) fs_context_menu_pos: egui::Pos2,

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

    // ── 回転リセット確認ダイアログ ─────────────────────────────
    pub(crate) show_rotation_reset_confirm: bool,

    // ── キャッシュ管理ポップアップ ───────────────────────────────
    pub(crate) show_cache_manager: bool,
    /// キャッシュ管理の「◯日以上古い」入力値
    pub(crate) cache_manager_days: u32,
    /// 開いたときに取得するキャッシュ統計: (フォルダ数, 合計バイト)
    pub(crate) cache_manager_stats: Option<(usize, u64)>,
    /// 削除後の結果メッセージ
    pub(crate) cache_manager_result: Option<String>,
    /// 「すべてのキャッシュを削除」の確認ステップ
    pub(crate) cache_manager_confirm_delete_all: bool,

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

    // ── 見開き表示 ──────────────────────────────────────────────
    /// 見開き DB (フォルダごとのモード永続化)
    pub(crate) spread_db: Option<crate::spread_db::SpreadDb>,
    /// 現在のフォルダの見開きモード
    pub(crate) spread_mode: crate::settings::SpreadMode,
    /// 見開きモード切替ポップアップ表示中
    pub(crate) spread_popup_open: bool,

    // ── スライドショー ────────────────────────────────────────────
    /// スライドショー再生中フラグ
    pub(crate) slideshow_playing: bool,
    /// 次の画像に切り替える時刻
    pub(crate) slideshow_next_at: std::time::Instant,

    // ── フルスクリーンビューポート ─────────────────────────────
    /// フルスクリーンビューポートが一度でも作成されたか
    pub(crate) fs_viewport_created: bool,
    /// フルスクリーンビューポートが現在表示中か（Visible+Focus 送信済み）
    pub(crate) fs_viewport_shown: bool,

    // ── 通常フルスクリーン ズーム/パン/任意回転 ──────────────
    /// 通常フルスクリーンのズーム倍率（1.0 = フィット）
    pub(crate) fs_zoom: f32,
    /// 通常フルスクリーンのパンオフセット（スクリーン座標系）
    pub(crate) fs_pan: egui::Vec2,
    /// 通常フルスクリーンのパンドラッグ開始状態
    pub(crate) fs_pan_drag_start: Option<(egui::Pos2, egui::Vec2)>,
    /// 任意角度回転（ラジアン、一時的・保存しない）
    pub(crate) fs_free_rotation: f32,
    /// 回転ドラッグ開始状態（開始位置, 開始時の回転角）
    pub(crate) fs_rotation_drag_start: Option<(egui::Pos2, f32)>,

    // ── 画像分析パネル ────────────────────────────────────────
    /// フルスクリーンで分析パネルを表示するか
    pub(crate) analysis_mode: bool,
    /// 分析パネル: マウス位置のピクセル色（画像座標系で取得）
    pub(crate) analysis_hover_color: Option<[u8; 4]>,
    /// 分析パネル: 右クリックで固定した比較色
    pub(crate) analysis_pinned_color: Option<[u8; 4]>,
    /// 分析パネル: グレースケール表示モード（G キー）
    pub(crate) analysis_grayscale: bool,
    /// 分析パネル: モザイクグリッド表示（M キー）
    pub(crate) analysis_mosaic_grid: bool,
    /// 分析パネル: 色差強調フィルターの倍率（0=無効, 2/5/10/20）
    pub(crate) analysis_filter_mag: u8,
    /// 分析パネル: ドラッグ計測ライン（開始点, 現在点：画像ピクセル座標, 修飾キー色インデックス）
    pub(crate) analysis_guide_drag: Option<(egui::Pos2, egui::Pos2, u8)>,
    /// 分析パネル: ズーム倍率（1.0 = フィット表示）
    pub(crate) analysis_zoom: f32,
    /// 分析パネル: パンオフセット（画像ピクセル座標系、画像中心からのズレ）
    pub(crate) analysis_pan: egui::Vec2,
    /// 分析パネル: ドラッグ中の開始オフセット
    pub(crate) analysis_pan_drag_start: Option<(egui::Pos2, egui::Vec2)>,
    /// 分析パネル: フィルター/グレースケールのキャッシュテクスチャ
    pub(crate) analysis_overlay_cache: Option<(egui::TextureHandle, u8, Option<[u8; 4]>, f32, egui::Vec2, usize)>,
    /// 分析パネル: ヒストグラムキャッシュ (zoom, pan, image_idx) → 結果
    pub(crate) analysis_hist_cache: Option<(f32, egui::Vec2, usize, [u32; 360], [u32; 256], [u32; 256])>,
    /// 分析パネル: SVマップキャッシュ
    pub(crate) analysis_sv_cache: Option<(f32, egui::Vec2, usize, egui::TextureHandle)>,

    // ── 起動時の前回フォルダ復元フラグ ──────────────────────────
    pub(crate) initialized: bool,

    // ── PDF パスワード管理 ───────────────────────────────────────
    pub(crate) pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
    pub(crate) show_pdf_password_dialog: bool,
    pub(crate) pdf_password_input: String,
    /// 「パスワードを保存する」チェックボックス (デフォルト OFF)
    pub(crate) pdf_password_save: bool,
    pub(crate) pdf_password_error: Option<String>,
    /// パスワード入力待ちの PDF パス
    pub(crate) pdf_password_pending_path: Option<PathBuf>,
    /// 現在開いている PDF のパスワード (セッション中キャッシュ)
    pub(crate) pdf_current_password: Option<String>,

    // ── PDF 非同期ロード ────────────────────────────────────────
    /// ページ列挙の非同期応答待ち: (pdf_path, password, receiver)
    pub(crate) pdf_enumerate_pending: Option<(
        PathBuf,
        Option<String>,
        mpsc::Receiver<std::io::Result<Vec<crate::pdf_loader::PdfPageEntry>>>,
    )>,

    // ── コンテキストメニュー: enumerate_handlers キャッシュ ────
    /// 拡張子ごとのシステム関連付けアプリ一覧キャッシュ (コンテキストメニュー開閉でクリア)
    pub(crate) cached_handlers: Option<(String, Vec<crate::open_with::AppHandler>)>,

    // ── 見開きペア解決用 nav_indices キャッシュ ────────────────
    /// フレーム内で build_nav_indices の結果をキャッシュ (items/visible_indices 変更でクリア)
    pub(crate) cached_nav_indices: Option<Vec<usize>>,

    // ── AI アップスケール ──────────────────────────────────────────
    /// AI ランタイム (ONNX Runtime)
    pub(crate) ai_runtime: Option<std::sync::Arc<crate::ai::runtime::AiRuntime>>,
    /// AI モデルマネージャ
    pub(crate) ai_model_manager: Option<std::sync::Arc<crate::ai::model_manager::ModelManager>>,
    /// AI アップスケール有効フラグ
    pub(crate) ai_upscale_enabled: bool,
    /// AI アップスケールモデルの手動オーバーライド (None = 自動)
    pub(crate) ai_upscale_model_override: Option<crate::ai::ModelKind>,
    /// アップスケール済みキャッシュ: item_idx → テクスチャ + ピクセルデータ
    pub(crate) ai_upscale_cache: std::collections::HashMap<usize, FsCacheEntry>,
    /// アップスケール処理中: item_idx → (キャンセルトークン, 受信チャネル)
    pub(crate) ai_upscale_pending: std::collections::HashMap<usize, (Arc<AtomicBool>, mpsc::Receiver<crate::ai::upscale::UpscaleResult>)>,
    /// 画像タイプ分類キャッシュ: item_idx → カテゴリ
    pub(crate) ai_classify_cache: std::collections::HashMap<usize, crate::ai::ImageCategory>,
    /// AI 補完 (inpainting) 有効フラグ
    pub(crate) ai_inpaint_active: bool,
    /// AI 補完の隙間幅 (論理ピクセル)
    pub(crate) ai_inpaint_gap_width: f32,
    /// AI 補完のトリム幅（左右の汚れ除去、ピクセル）
    pub(crate) ai_inpaint_trim: f32,
    /// AI 補完のドラッグ状態: (開始 X, 開始 Y, 開始時の幅, 開始時のトリム)
    pub(crate) ai_inpaint_drag: Option<(f32, f32, f32, f32)>,
    /// AI モデルセットアップダイアログの表示フラグ
    pub(crate) show_ai_model_setup: bool,
    /// バージョン情報ダイアログ
    pub(crate) show_about_dialog: bool,
    /// AI アップスケールが失敗した idx の集合（リトライ防止）
    pub(crate) ai_upscale_failed: std::collections::HashSet<usize>,
    /// AI ステータス表示の完了時刻（全処理完了後に記録、一定時間後に非表示）
    pub(crate) ai_status_done_at: Option<std::time::Instant>,
    /// AI 補完キャッシュ: (left_idx, right_idx, gap_width) → テクスチャ
    pub(crate) ai_inpaint_cache: std::collections::HashMap<(usize, usize, u32, u32), egui::TextureHandle>,
    /// AI 補完が失敗したキーの集合（リトライ防止）
    pub(crate) ai_inpaint_failed: std::collections::HashSet<(usize, usize, u32, u32)>,
    /// AI 補完処理中: (キャンセルトークン, 受信チャネル, キャッシュキー)
    pub(crate) ai_inpaint_pending: Option<(Arc<AtomicBool>, mpsc::Receiver<crate::ai::inpaint::InpaintResult>, (usize, usize, u32, u32))>,

    // ── 画像補正 ──────────────────────────────────────────────────
    /// 補正パネル表示フラグ
    pub(crate) adjustment_mode: bool,
    /// 現フォルダ/ZIP/PDF の 4 プリセット
    pub(crate) adjustment_presets: crate::adjustment::AdjustPresets,
    /// 現在アクティブなプリセット番号 (0-3)。None = 補正なし。
    pub(crate) adjustment_active_preset: Option<u8>,
    /// ページごとのプリセット割り当て: item_idx → preset_idx
    pub(crate) adjustment_page_preset: std::collections::HashMap<usize, u8>,
    /// 補正済み画像キャッシュ: item_idx → テクスチャ + ピクセルデータ
    pub(crate) adjustment_cache: std::collections::HashMap<usize, FsCacheEntry>,
    /// スライダードラッグ中の低解像度プレビュー
    pub(crate) adjustment_preview_tex: Option<egui::TextureHandle>,
    /// 前回プレビュー生成時のパラメータ（変更検出用）
    pub(crate) adjustment_preview_params: Option<crate::adjustment::AdjustParams>,
    /// スライダードラッグ中フラグ
    pub(crate) adjustment_dragging: bool,
    /// 補正 DB ハンドル
    pub(crate) adjustment_db: Option<crate::adjustment_db::AdjustmentDb>,
    /// バックグラウンドシャープネス処理中: (item_idx, receiver)
    pub(crate) adjustment_pending: Option<(usize, mpsc::Receiver<egui::ColorImage>)>,
    /// シャープネス適用済みの idx 集合（再適用防止）
    pub(crate) adjustment_sharpened: std::collections::HashSet<usize>,
}

impl Default for App {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let settings = crate::settings::Settings::load();
        let ai_upscale_enabled = settings.ai_upscale_enabled;
        let ai_upscale_model_override = settings.ai_upscale_model_override.as_deref()
            .and_then(crate::ai::ModelKind::from_str);
        let ai_inpaint_active = settings.ai_inpaint_active;
        let ai_inpaint_gap_width = settings.ai_inpaint_gap_width as f32;
        let ai_inpaint_trim = settings.ai_inpaint_trim as f32;
        Self {
            address: String::new(),
            current_folder: None,
            items: Vec::new(),
            thumbnails: Vec::new(),
            selected: None,
            settings,
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
            heavy_io_queue: None,
            requested: std::collections::HashMap::new(),
            keep_range: (0, 0),
            keep_start_shared: Arc::new(AtomicUsize::new(0)),
            keep_end_shared: Arc::new(AtomicUsize::new(0)),
            texture_backlog: Vec::new(),
            folder_nav_pending: None,
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
            pref_state: None,
            checked: std::collections::HashSet::new(),
            context_menu_idx: None,
            context_menu_pos: egui::Pos2::ZERO,
            fs_secondary_press_start: None,
            fs_context_menu_idx: None,
            fs_context_menu_pos: egui::Pos2::ZERO,
            show_delete_confirm: false,
            delete_targets: Vec::new(),
            pending_reload: false,
            select_after_load: None,
            video_thumb_overrides: std::collections::HashMap::new(),
            show_rotation_reset_confirm: false,
            show_cache_manager: false,
            cache_manager_days: 90,
            cache_manager_stats: None,
            cache_manager_result: None,
            cache_manager_confirm_delete_all: false,
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
            spread_db: crate::spread_db::SpreadDb::open().ok(),
            spread_mode: crate::settings::SpreadMode::default(),
            spread_popup_open: false,
            slideshow_playing: false,
            slideshow_next_at: std::time::Instant::now(),
            fs_viewport_created: false,
            fs_viewport_shown: false,
            fs_zoom: 1.0,
            fs_pan: egui::Vec2::ZERO,
            fs_pan_drag_start: None,
            fs_free_rotation: 0.0,
            fs_rotation_drag_start: None,
            analysis_mode: false,
            analysis_hover_color: None,
            analysis_pinned_color: None,
            analysis_grayscale: false,
            analysis_mosaic_grid: false,
            analysis_filter_mag: 0,
            analysis_guide_drag: None,
            analysis_zoom: 1.0,
            analysis_pan: egui::Vec2::ZERO,
            analysis_pan_drag_start: None,
            analysis_overlay_cache: None,
            analysis_hist_cache: None,
            analysis_sv_cache: None,
            initialized: false,
            pdf_passwords: crate::pdf_passwords::PdfPasswordStore::load(),
            show_pdf_password_dialog: false,
            pdf_password_input: String::new(),
            pdf_password_save: false,
            pdf_password_error: None,
            pdf_password_pending_path: None,
            pdf_current_password: None,
            pdf_enumerate_pending: None,
            cached_handlers: None,
            cached_nav_indices: None,

            // AI (settings から復元)
            ai_runtime: None,
            ai_model_manager: None,
            ai_upscale_enabled,
            ai_upscale_model_override,
            ai_upscale_cache: std::collections::HashMap::new(),
            ai_upscale_pending: std::collections::HashMap::new(),
            ai_classify_cache: std::collections::HashMap::new(),
            ai_inpaint_active,
            ai_inpaint_gap_width,
            ai_inpaint_trim,
            ai_inpaint_drag: None,
            show_ai_model_setup: false,
            show_about_dialog: false,
            ai_upscale_failed: std::collections::HashSet::new(),
            ai_status_done_at: None,
            ai_inpaint_cache: std::collections::HashMap::new(),
            ai_inpaint_failed: std::collections::HashSet::new(),
            ai_inpaint_pending: None,

            // 画像補正
            adjustment_mode: false,
            adjustment_presets: crate::adjustment::AdjustPresets::default(),
            adjustment_active_preset: None,
            adjustment_page_preset: std::collections::HashMap::new(),
            adjustment_cache: std::collections::HashMap::new(),
            adjustment_preview_tex: None,
            adjustment_preview_params: None,
            adjustment_dragging: false,
            adjustment_db: crate::adjustment_db::AdjustmentDb::open().ok(),
            adjustment_pending: None,
            adjustment_sharpened: std::collections::HashSet::new(),
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
            || self.show_cache_manager
            || self.show_delete_confirm
            || self.show_rotation_reset_confirm
            || self.show_pdf_password_dialog
            || self.context_menu_idx.is_some()
    }

    pub fn load_folder(&mut self, path: PathBuf) {
        // パスが .zip / .pdf ファイルなら仮想フォルダとして開く
        if path.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            if ext == "zip" {
                self.load_zip_as_folder(path);
                return;
            }
            if ext == "pdf" {
                self.load_pdf_as_folder(path);
                return;
            }
        }

        crate::logger::log(format!("=== load_folder: {} ===", path.display()));

        // ── ディレクトリ走査（画像はメタデータも収集）────────────────
        let mut folders: Vec<GridItem> = Vec::new();
        // フォルダアイテムごとのメタデータ (ZipFile/PdfFile はサムネイルロードに必要)
        let mut folder_metas: Vec<Option<(i64, i64)>> = Vec::new();
        // (path, is_video, mtime, file_size)
        let mut all_media: Vec<(PathBuf, bool, i64, i64)> = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let meta = entry.metadata().ok();
                    let mtime = meta.as_ref().map_or(0, |m| crate::ui_helpers::mtime_secs(m));
                    folders.push(GridItem::Folder(p));
                    folder_metas.push(Some((mtime, 0)));
                } else if is_apple_double(&p) {
                    // macOS/iPhone AppleDouble メタデータ — スキップ
                } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    let ext_lower = ext.to_ascii_lowercase();
                    let meta = entry.metadata().ok();
                    let mtime = meta.as_ref().map_or(0, |m| crate::ui_helpers::mtime_secs(m));
                    let file_size = meta.map_or(0, |m| m.len() as i64);
                    if SUPPORTED_EXTENSIONS.contains(&ext_lower.as_str()) {
                        all_media.push((p, false, mtime, file_size));
                    } else if SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_lower.as_str()) {
                        all_media.push((p, true, mtime, file_size));
                    } else if ext_lower == "zip" {
                        folders.push(GridItem::ZipFile(p));
                        folder_metas.push(Some((mtime, file_size)));
                    } else if ext_lower == "pdf" {
                        folders.push(GridItem::PdfFile(p));
                        folder_metas.push(Some((mtime, file_size)));
                    }
                }
            }
        }

        {
            // folders と folder_metas を同じ順序でソート
            let mut paired: Vec<_> = folders.into_iter().zip(folder_metas).collect();
            paired.sort_by(|(a, _), (b, _)| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
            let (f, m): (Vec<_>, Vec<_>) = paired.into_iter().unzip();
            folders = f;
            folder_metas = m;
        }
        let sort = self.settings.sort_order;
        all_media.sort_by(|(a, _, a_mt, _), (b, _, b_mt, _)| {
            let an = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let bn = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
            sort.compare(an, *a_mt, bn, *b_mt, natural_sort_key)
        });

        // ── 同名ファイルフィルタ ─────────────────────────────────────
        self.video_thumb_overrides.clear();
        self.apply_duplicate_filters(&mut folders, &mut folder_metas, &mut all_media);

        // items: フォルダ先頭 → メディア（画像・動画を名前順混在）
        let folder_count = folders.len();
        let mut items: Vec<GridItem> = folders;
        let mut image_metas: Vec<Option<(i64, i64)>> = folder_metas;
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
                GridItem::ZipFile(p) => {
                    let fname = p.file_name()?.to_str()?;
                    Some(format!("{}{fname}", CACHE_KEY_ZIP))
                }
                GridItem::PdfFile(p) => {
                    let fname = p.file_name()?.to_str()?;
                    Some(format!("{}{fname}", CACHE_KEY_PDF))
                }
                GridItem::Folder(p) => {
                    let fname = p.file_name()?.to_str()?;
                    Some(format!("{}{fname}", CACHE_KEY_FOLDER))
                }
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

    /// PDF ファイルを仮想フォルダとして開く (非同期)。
    ///
    /// ワーカーにページ列挙リクエストを送り、即座に return する。
    /// 結果は `poll_pdf_enumerate` が次フレーム以降にポーリングして処理する。
    /// パスワード付き PDF の場合はダイアログで入力を求める。
    pub fn load_pdf_as_folder(&mut self, pdf_path: PathBuf) {
        crate::logger::log(format!("=== load_pdf_as_folder: {} ===", pdf_path.display()));

        // 旧サムネイルワーカーを即座にキャンセルして PDF ワーカーキューの渋滞を防ぐ。
        // start_loading_items は enumerate 完了後に呼ばれるため、ここで先行キャンセルする。
        self.cancel_token.store(true, Ordering::Relaxed);

        // ── パスワード確認 ──
        let password: Option<String> = self
            .pdf_passwords
            .get(&pdf_path)
            .or_else(|| self.pdf_current_password.clone());

        // パスワードチェックも非同期化したいが、ダイアログ表示のフローが複雑になるため
        // ここでは簡易判定: 保存済みパスワードがなければ非同期で check_password を含めて
        // enumerate を試みる。パスワードエラーは結果受信時にハンドルする。

        // ── 非同期でページ列挙をリクエスト ──
        let rx = crate::pdf_loader::enumerate_pages_async(
            &pdf_path,
            password.as_deref(),
        );
        self.pdf_enumerate_pending = Some((pdf_path.clone(), password, rx));

        // アドレスバーを即座に更新 (ローディング中であることを示す)
        self.address = pdf_path.to_string_lossy().to_string();
    }

    /// PDF ページ列挙の非同期応答をポーリングする。
    /// 毎フレーム `update()` から呼び出す。
    pub(crate) fn poll_pdf_enumerate(&mut self) {
        let Some((ref pdf_path, _, ref rx)) = self.pdf_enumerate_pending else {
            return;
        };

        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return, // まだ結果が来ていない
            Err(mpsc::TryRecvError::Disconnected) => {
                // ワーカーが切断 (通常起きない)
                crate::logger::log("  pdf enumerate: worker disconnected");
                let path = pdf_path.clone();
                self.pdf_enumerate_pending = None;
                self.start_loading_items(
                    path, Vec::new(), Vec::new(),
                    std::collections::HashSet::new(), Vec::new(),
                );
                return;
            }
        };

        let (pdf_path, password, _) = self.pdf_enumerate_pending.take().unwrap();

        match result {
            Ok(pages) => {
                crate::logger::log(format!("  pdf: {} pages", pages.len()));
                self.pdf_current_password = password;

                let mut items: Vec<GridItem> = Vec::new();
                let mut image_metas: Vec<Option<(i64, i64)>> = Vec::new();
                let mut existing_keys: std::collections::HashSet<String> =
                    std::collections::HashSet::new();

                for page in &pages {
                    let key = crate::grid_item::pdf_page_cache_key(page.page_num);
                    existing_keys.insert(key);
                    items.push(GridItem::PdfPage {
                        pdf_path: pdf_path.clone(),
                        page_num: page.page_num,
                    });
                    image_metas.push(Some((page.mtime, page.file_size as i64)));
                }

                self.start_loading_items(pdf_path, items, image_metas, existing_keys, Vec::new());
            }
            Err(e) => {
                let err_msg = format!("{e}");
                // パスワードエラーかどうかを判定 (エラーメッセージに "Password" が含まれる)
                if err_msg.contains("Password") || err_msg.contains("password") {
                    if password.is_none() {
                        // パスワードが必要 → ダイアログ表示
                        self.pdf_password_pending_path = Some(pdf_path);
                        self.show_pdf_password_dialog = true;
                        self.pdf_password_input.clear();
                        self.pdf_password_error = None;
                        self.pdf_password_save = false;
                        return;
                    }
                }
                crate::logger::log(format!("  pdf enumerate failed: {e}"));
                self.start_loading_items(
                    pdf_path, Vec::new(), Vec::new(),
                    std::collections::HashSet::new(), Vec::new(),
                );
            }
        }
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

        // 進行中のフォルダナビゲーションをキャンセル
        // (他の経路でフォルダが変更された場合に不要な結果を破棄する)
        if let Some((cancel_nav, _)) = self.folder_nav_pending.take() {
            cancel_nav.store(true, Ordering::Relaxed);
        }

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
        self.texture_backlog.clear();
        self.keep_range = (0, 0);
        self.rebuild_visible_indices();
        self.metadata_cache.clear();
        self.exif_cache.clear();
        self.checked.clear();
        self.rotation_cache.clear();
        // 見開きモード: DB から読み込み、なければデフォルト値
        self.spread_mode = self.spread_db.as_ref()
            .and_then(|db| db.get(&source_path))
            .unwrap_or(self.settings.default_spread_mode);
        self.spread_popup_open = false;
        self.search_filter = None;
        self.search_query.clear();

        // 画像補正: DB からプリセット読み込み + ページプリセット復元
        self.adjustment_cache.clear();
        self.adjustment_sharpened.clear();
        self.adjustment_page_preset.clear();
        self.adjustment_preview_tex = None;
        self.adjustment_dragging = false;
        self.adjustment_active_preset = None;
        self.adjustment_mode = false;
        if let Some((_, _)) = self.adjustment_pending.take() {}
        self.adjustment_presets = self.adjustment_db.as_ref()
            .and_then(|db| db.get_presets(&source_path))
            .unwrap_or_default();
        // ページごとのプリセット割り当てを DB から復元 → item_idx にマッピング
        if let Some(db) = &self.adjustment_db {
            let prefix = crate::adjustment_db::normalize_path(&source_path);
            let page_map = db.load_page_presets(&prefix);
            if !page_map.is_empty() {
                for idx in 0..self.items.len() {
                    if let Some(key) = self.page_path_key(idx) {
                        if let Some(&preset_idx) = page_map.get(&key) {
                            self.adjustment_page_preset.insert(idx, preset_idx);
                        }
                    }
                }
            }
        }
        // visible_indices はアイテム設定後 (下の行) に再計算される

        // ── カタログを開く + cache_map ロード + 削除掃除 ──
        let cache_dir = crate::catalog::default_cache_dir();
        let catalog_arc: Option<Arc<crate::catalog::CatalogDb>> =
            crate::catalog::CatalogDb::open(&cache_dir, &source_path)
                .map_err(|e| crate::logger::log(format!("  catalog open failed: {e}")))
                .ok()
                .map(Arc::new);

        let cache_map: Arc<std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>> =
            Arc::new(std::sync::RwLock::new(
                catalog_arc
                    .as_ref()
                    .and_then(|c| c.load_all().ok())
                    .unwrap_or_default(),
            ));
        crate::logger::log(format!("  catalog: {} entries in DB", cache_map.read().unwrap().len()));

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
        let heavy_io_queue: Arc<Mutex<Vec<LoadRequest>>> = Arc::new(Mutex::new(Vec::new()));
        self.reload_queue = Some(Arc::clone(&reload_queue));
        self.heavy_io_queue = Some(Arc::clone(&heavy_io_queue));

        self.spawn_thumbnail_workers(
            &tx,
            Arc::clone(&cancel),
            reload_queue,
            heavy_io_queue,
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
        heavy_io_queue: Arc<Mutex<Vec<LoadRequest>>>,
        cache_map: Arc<std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>>,
        catalog_arc: Option<Arc<crate::catalog::CatalogDb>>,
    ) {
        let total_threads = self.settings.parallelism.thread_count();
        // I/O ワーカー数: 2 本 (HDD シーク競合と並列性のバランス)
        // ただし全体が 4 本以下なら 1 本に制限
        let io_threads = if total_threads <= 4 { 1 } else { 2 };
        let regular_threads = total_threads.saturating_sub(io_threads).max(1);
        let thumb_px = self.settings.thumb_px;
        let thumb_quality = self.settings.thumb_quality;
        let cache_decision = CacheDecision::from_settings(&self.settings);
        let scroll_hint = Arc::clone(&self.scroll_hint);
        let display_px_shared = Arc::clone(&self.display_px_shared);
        let stats = Arc::clone(&self.stats);
        let cache_gen_done = Arc::clone(&self.cache_gen_done);
        let keep_start_shared = Arc::clone(&self.keep_start_shared);
        let keep_end_shared = Arc::clone(&self.keep_end_shared);

        crate::logger::log(format!(
            "  spawning {} regular + {} I/O workers",
            regular_threads, io_threads,
        ));

        // ── 共通のワーカーループ本体 ──
        // queue を受け取り、priority 順に取り出して process_load_request を呼ぶ。
        let spawn_worker = |worker_idx: usize, prefix: &str, queue: Arc<Mutex<Vec<LoadRequest>>>| {
            let tx_w = tx.clone();
            let cancel_w = Arc::clone(&cancel);
            let hint_w = Arc::clone(&scroll_hint);
            let cache_map_w = Arc::clone(&cache_map);
            let catalog_w = catalog_arc.clone();
            let done_w = Arc::clone(&cache_gen_done);
            let display_px_w = Arc::clone(&display_px_shared);
            let stats_w = Arc::clone(&stats);
            let ks_w = Arc::clone(&keep_start_shared);
            let ke_w = Arc::clone(&keep_end_shared);
            let tag = format!("{prefix}{worker_idx}");

            std::thread::spawn(move || {
                crate::logger::log(format!("  {tag} started"));
                loop {
                    if cancel_w.load(Ordering::Relaxed) { break; }

                    // priority (可視範囲) を最優先、次に scroll_hint に近い順
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
                                    let tier: usize = if r.priority { 0 } else { 1 };
                                    let i = r.idx;
                                    let dist = if i < vis { vis - i } else { i - vis };
                                    (tier, dist)
                                })
                                .map(|(pos, _)| pos)
                                .unwrap();
                            Some(q.swap_remove(best))
                        }
                    };

                    match req_opt {
                        Some(req) => {
                            if cancel_w.load(Ordering::Relaxed) { break; }
                            let ks = ks_w.load(Ordering::Relaxed);
                            let ke = ke_w.load(Ordering::Relaxed);
                            if req.idx < ks || req.idx >= ke {
                                crate::logger::log(format!(
                                    "  {tag} SKIP idx={:>4} (out of keep [{ks}..{ke}))  {}",
                                    req.idx,
                                    req.path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                                ));
                                continue;
                            }
                            let vis = hint_w.load(Ordering::Relaxed);
                            let dist = if req.idx < vis { vis - req.idx } else { req.idx - vis };
                            crate::logger::log(format!(
                                "  {tag} pick idx={:>4} pri={} dist={dist:>4}  {}",
                                req.idx,
                                if req.priority { "H" } else { "L" },
                                req.path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                            ));
                            let display_px = display_px_w.load(Ordering::Relaxed);
                            process_load_request(
                                &req, &cache_map_w, &tx_w, catalog_w.as_deref(),
                                thumb_px, thumb_quality, display_px, cache_decision, &done_w,
                                &stats_w,
                                Some(&cancel_w),
                                &ks_w, &ke_w,
                            );
                        }
                        None => {
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                    }
                }
                crate::logger::log(format!("  {tag} stopped"));
            });
        };

        // 通常ワーカー: reload_queue (Image, ZipImage, PdfPage)
        for i in 0..regular_threads {
            spawn_worker(i, "w", Arc::clone(&reload_queue));
        }
        // I/O ワーカー: heavy_io_queue (ZipFile, PdfFile, Folder)
        for i in 0..io_threads {
            spawn_worker(i, "io", Arc::clone(&heavy_io_queue));
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
                let stem = stem_lower(&path);
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
        // 1 フレームあたりのテクスチャ生成数を制限する。
        // load_texture は GPU テクスチャアップロードを伴い、1 枚 0.5-2 ms かかる。
        // キャッシュヒット時にワーカー全員が一気に結果を返すと 1 フレームで
        // 数十枚の upload が走りフレーム落ちするため、上限を設ける。
        // 上限超過分は texture_backlog に ColorImage のまま保持し次フレームで処理する。
        const MAX_TEXTURES_PER_FRAME: u32 = 8;
        let mut textures_created = 0u32;
        let mut received = 0u32;
        let (keep_start, keep_end) = self.keep_range;

        // バックログ + チャネルから受信した結果を統合して処理する。
        // バックログを先に処理（既にデコード済みなので優先）。
        let backlog = std::mem::take(&mut self.texture_backlog);
        let drain = backlog.into_iter().chain(
            std::iter::from_fn(|| self.rx.try_recv().ok())
        );

        for (i, color_image_opt, from_cache, source_dims) in drain {
            if i >= self.thumbnails.len() {
                self.requested.remove(&i);
                continue;
            }

            let in_keep_range = i >= keep_start && i < keep_end;
            match color_image_opt {
                Some(color_image) => {
                    if in_keep_range && textures_created < MAX_TEXTURES_PER_FRAME {
                        self.requested.remove(&i);
                        let [w, h] = color_image.size;
                        let rendered_at_px = w.max(h) as u32;
                        let handle = ctx.load_texture(
                            format!("thumb_{i}"),
                            color_image,
                            egui::TextureOptions::LINEAR,
                        );
                        self.thumbnails[i] = ThumbnailState::Loaded {
                            tex: handle,
                            from_cache,
                            rendered_at_px,
                            source_dims,
                        };
                        textures_created += 1;
                    } else if in_keep_range {
                        // 上限到達だが keep_range 内: 次フレームに持ち越す。
                        // requested は除去しない (重複リクエスト防止)
                        self.texture_backlog.push((i, Some(color_image), from_cache, source_dims));
                    } else {
                        // 範囲外: ColorImage を drop し Evicted にしておく
                        self.requested.remove(&i);
                        self.thumbnails[i] = ThumbnailState::Evicted;
                    }
                }
                None => {
                    self.requested.remove(&i);
                    self.thumbnails[i] = ThumbnailState::Failed;
                }
            }
            received += 1;
        }
        if received > 0 || !self.texture_backlog.is_empty() {
            crate::logger::log(format!(
                "  [main] poll_thumbnails: received {received} ({textures_created} textures, {} backlog)",
                self.texture_backlog.len()
            ));
            ctx.request_repaint();
        }
    }

    /// 段階 B: ページ単位先読み + eviction のメインロジック。
    /// 段階 D: VRAM 安全ネット (上限超過時に keep_range を縮小)。
    ///
    /// 毎フレーム呼ぶ想定。現在のスクロール位置から keep_range を算出し、
    /// 範囲外の Loaded を Evicted 化し、範囲内の Pending/Evicted を reload_queue に push する。
    fn update_keep_range_and_requests(&mut self, frame_t0: std::time::Instant) {
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
        self.keep_start_shared.store(keep_start, Ordering::Relaxed);
        self.keep_end_shared.store(keep_end, Ordering::Relaxed);

        // (1) 範囲外の Loaded を Evicted にする (TextureHandle を drop)
        //     動画サムネイルは一度ロードしたら維持する (別パスのため再要求できない)
        //     同時に requested からも除去する (ワーカーが処理中のものは結果受信時に
        //     keep_range 外判定で Evicted になるが、requested に残っていると
        //     同じ idx の再リクエストがブロックされてしまう)
        let t1 = frame_t0.elapsed();
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
            self.requested.remove(&i);
        }
        let t2 = frame_t0.elapsed();

        // (2) reload_queue 内の keep_range 外リクエストを除去し、
        //     範囲内の Pending/Evicted を新たに push する。
        //     スクロール中にキューに溜まった古いリクエストをワーカーが無駄に
        //     処理するのを防ぎ、新しい可視領域のリクエストを優先させる。
        //
        //     可視範囲 (1 ページ分) のリクエストは priority=true でマークし、
        //     ワーカーが先読み要求より常に先に処理するようにする。
        let Some(queue) = self.reload_queue.clone() else { return; };

        // 可視範囲の raw index 範囲を計算 (1 ページ分 + 上下 1 行のマージン)
        let vis_visible_start = vis_first.saturating_sub(cols);
        let vis_visible_end = vis_first
            .saturating_add(items_per_page + cols)
            .min(vis_count);
        let visible_raw_start = self
            .visible_indices
            .get(vis_visible_start)
            .copied()
            .unwrap_or(0);
        let visible_raw_end = self
            .visible_indices
            .get(vis_visible_end.saturating_sub(1))
            .copied()
            .map(|i| i + 1)
            .unwrap_or(total)
            .min(total);

        // 通常リクエストと重い I/O リクエストを分けて収集
        let mut new_regular: Vec<LoadRequest> = Vec::new();
        let mut new_heavy: Vec<LoadRequest> = Vec::new();
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
            let Some((mtime, file_size)) = self.image_metas.get(i).copied().flatten() else {
                continue;
            };
            let Some(mut req) = self.items.get(i).and_then(|item| {
                make_load_request(item, i, mtime, file_size, false, self.pdf_current_password.as_deref(), Some(self.settings.folder_thumb_sort), self.settings.folder_thumb_depth)
            }) else {
                continue;
            };
            req.priority = i >= visible_raw_start && i < visible_raw_end;
            // ZipFile / PdfFile / Folder → heavy_io_queue、それ以外 → reload_queue
            let is_heavy = matches!(
                self.items.get(i),
                Some(GridItem::ZipFile(_) | GridItem::PdfFile(_) | GridItem::Folder(_))
            );
            if is_heavy {
                new_heavy.push(req);
            } else {
                new_regular.push(req);
            }
        }
        let new_hi = new_regular.iter().chain(new_heavy.iter()).filter(|r| r.priority).count();
        let new_lo = new_regular.len() + new_heavy.len() - new_hi;
        let t3 = frame_t0.elapsed();
        {
            let mut q = queue.lock().unwrap();
            q.retain(|r| r.idx >= keep_start && r.idx < keep_end);
            for r in q.iter_mut() {
                r.priority = r.idx >= visible_raw_start && r.idx < visible_raw_end;
            }
            let _q_before = q.len();
            for r in new_regular {
                self.requested.insert(r.idx, false);
                q.push(r);
            }
        }
        // heavy_io_queue にも同様に push
        if let Some(hq) = self.heavy_io_queue.clone() {
            let mut q = hq.lock().unwrap();
            q.retain(|r| r.idx >= keep_start && r.idx < keep_end);
            for r in q.iter_mut() {
                r.priority = r.idx >= visible_raw_start && r.idx < visible_raw_end;
            }
            for r in new_heavy {
                self.requested.insert(r.idx, false);
                q.push(r);
            }
        }
        if new_hi > 0 || new_lo > 0 {
            crate::logger::log(format!(
                "  [queue] push +{new_hi}H +{new_lo}L  keep=[{keep_start}..{keep_end})  vis=[{visible_raw_start}..{visible_raw_end})  requested={}",
                self.requested.len(),
            ));
        }
        let t4 = frame_t0.elapsed();

        // (3) 段階 E: アイドル時の画質アップグレード
        self.enqueue_idle_upgrades(keep_start, keep_end);
        let t5 = frame_t0.elapsed();

        // (4) 進捗ピーク値の更新 (プログレスバー表示用)
        self.update_progress_peaks();
        let t6 = frame_t0.elapsed();

        if (t6 - t1).as_millis() > 5 {
            crate::logger::log(format!(
                "    [keep detail] evict={:.1}ms scan={:.1}ms lock+push={:.1}ms idle={:.1}ms peaks={:.1}ms",
                (t2 - t1).as_secs_f64() * 1000.0,
                (t3 - t2).as_secs_f64() * 1000.0,
                (t4 - t3).as_secs_f64() * 1000.0,
                (t5 - t4).as_secs_f64() * 1000.0,
                (t6 - t5).as_secs_f64() * 1000.0,
            ));
        }
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
                make_load_request(item, i, mtime, file_size, true, self.pdf_current_password.as_deref(), Some(self.settings.folder_thumb_sort), self.settings.folder_thumb_depth)
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
    ///
    /// `requested` マップは queue に push した時点で insert され、
    /// 結果を受信した時点で remove されるので、キュー待ち + ワーカー処理中の
    /// 全件を正確に反映している。queue を別途カウントすると二重計上になるため
    /// `requested` のみを集計する。
    fn count_pending(&self) -> (usize, usize) {
        let (keep_start, keep_end) = self.keep_range;
        let (mut in_normal, mut in_upgrade) = (0usize, 0usize);
        for (&idx, &is_upgrade) in &self.requested {
            // keep_range 外のリクエストは「処理中だがスクロールで不要になった」もの。
            // 進捗バーに含めない (ワーカー完了時に除去される)。
            if idx < keep_start || idx >= keep_end {
                continue;
            }
            if is_upgrade { in_upgrade += 1; } else { in_normal += 1; }
        }
        (in_normal, in_upgrade)
    }

    /// 現在の要求状況からプログレスバーのピーク値を更新する。
    fn update_progress_peaks(&mut self) {
        let backlog_count = self.texture_backlog.len();
        let (cur_normal_raw, cur_upgrade) = self.count_pending();
        // backlog 内のアイテムは requested に残っており count_pending でカウント
        // 済みだが、実際にはデコード完了済み。pending として見せると分母が膨らむので
        // 差し引く (ただし 0 以下にはしない)。
        let cur_normal = cur_normal_raw.saturating_sub(backlog_count);

        if cur_normal == 0 {
            self.progress_normal_peak = 0;
        } else if cur_normal > self.progress_normal_peak {
            // 新しいスクロール位置で新規リクエストが発生した場合は
            // peak を現在値にリセットする (古い peak が蓄積し続けるのを防ぐ)
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
                                | Some(GridItem::ZipImage { .. })
                                | Some(GridItem::PdfPage { .. }) => {
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
                            | Some(GridItem::ZipImage { .. })
                            | Some(GridItem::PdfPage { .. }) => {
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
                        Some(GridItem::ZipFile(p)) | Some(GridItem::PdfFile(p)) => {
                            return Some(p.clone());
                        }
                        Some(GridItem::Image(_))
                        | Some(GridItem::ZipImage { .. })
                        | Some(GridItem::ZipSeparator { .. })
                        | Some(GridItem::PdfPage { .. }) => self.open_fullscreen(idx),
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
        // バックグラウンドスレッドで navigate_folder_with_skip を実行し、
        // 結果は poll_folder_nav で非同期に受信する。
        if ctrl_down {
            if let Some(ref cur) = self.current_folder.clone() {
                self.start_folder_nav(cur.clone(), true);
            }
        }

        // Ctrl+↑: 深さ優先で前のフォルダへ（画像なしはスキップ）
        if ctrl_up {
            if let Some(ref cur) = self.current_folder.clone() {
                self.start_folder_nav(cur.clone(), false);
            }
        }

        None
    }

    /// Ctrl+↑↓ のフォルダナビゲーションをバックグラウンドスレッドで開始する。
    /// `navigate_folder_with_skip` はフォルダツリーの DFS 走査 + `folder_should_stop`
    /// (`read_dir`) を行うためディスク I/O を伴い、HDD では 20-120ms かかる。
    /// UI スレッドをブロックしないよう、結果は `poll_folder_nav` で非同期に受信する。
    fn start_folder_nav(&mut self, current: PathBuf, forward: bool) {
        // 既存のナビゲーションをキャンセル (連打対応)
        if let Some((cancel, _)) = self.folder_nav_pending.take() {
            cancel.store(true, Ordering::Relaxed);
        }

        let skip_limit = self.settings.folder_skip_limit;
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_w = Arc::clone(&cancel);

        std::thread::spawn(move || {
            let result = if forward {
                navigate_folder_with_skip(&current, next_folder_dfs, skip_limit)
            } else {
                navigate_folder_with_skip(&current, prev_folder_dfs, skip_limit)
            };
            if !cancel_w.load(Ordering::Relaxed) {
                let _ = tx.send(result);
            }
        });

        self.folder_nav_pending = Some((cancel, rx));
    }

    /// バックグラウンドフォルダナビゲーションの結果を非同期にポーリングする。
    /// 結果が到着していれば `Some(path)` を返し、未完了なら `None` を返す。
    fn poll_folder_nav(&mut self) -> Option<PathBuf> {
        let Some((_, ref rx)) = self.folder_nav_pending else { return None; };
        match rx.try_recv() {
            Ok(result) => {
                self.folder_nav_pending = None;
                result
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.folder_nav_pending = None;
                None
            }
        }
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
                // Ctrl+ホイール: 列数を増減（1〜10 の範囲）
                let delta = -scroll_delta_y.signum() as i32;
                let new_cols = (self.settings.grid_cols as i32 + delta).clamp(crate::settings::MIN_GRID_COLS as i32, crate::settings::MAX_GRID_COLS as i32) as usize;
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

    /// ウィンドウ位置を記録する（最小化・最大化中は更新しない）。
    fn track_window_rect(&mut self, ctx: &egui::Context) {
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

        if outer_rect.is_none() && self.last_outer_rect.is_none() {
            crate::logger::log(format!(
                "[viewport] outer_rect=None  inner_rect={:?}  pixels_per_point={pixels_per_point:.2}",
                inner_rect.map(|r| format!("pos=({:.0},{:.0}) size={:.0}x{:.0}",
                    r.min.x, r.min.y, r.width(), r.height()))
            ));
        }

        let best_rect = outer_rect.or(inner_rect);
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

    /// Ctrl+C / Ctrl+X / Ctrl+V ショートカットを処理する。
    fn handle_clipboard_shortcuts(&mut self, ctx: &egui::Context) {
        let main_focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if !main_focused || self.any_dialog_open() || self.address_has_focus
            || self.search_has_focus || self.fullscreen_idx.is_some()
        {
            return;
        }

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

        let ctrl_v = {
            #[cfg(windows)]
            {
                let ctrl = unsafe {
                    windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x11)
                };
                let v = unsafe {
                    windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x56)
                };
                (ctrl & (0x8000u16 as i16)) != 0
                    && (v & (0x8000u16 as i16)) != 0
                    && (v & 1) != 0
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

    // -----------------------------------------------------------------------
    // フルスクリーン表示
    // -----------------------------------------------------------------------

    /// フルスクリーン表示を開始する。
    /// キャッシュ済みなら即座に表示し、そうでなければ読み込みを開始する。
    /// 動画アイテムの場合はサムネイル＋再生ボタンを表示するだけで読み込みは不要。
    pub fn open_fullscreen(&mut self, idx: usize) {
        crate::logger::log(format!("=== open_fullscreen: idx={idx} ==="));
        self.fullscreen_idx = Some(idx);

        // ページに割り当て済みのプリセットがあれば復元
        if let Some(&pi) = self.adjustment_page_preset.get(&idx) {
            self.adjustment_active_preset = Some(pi);
        }

        // 画像切り替え時にズーム/パン/キャッシュをリセット
        self.analysis_zoom = 1.0;
        self.analysis_pan = egui::Vec2::ZERO;
        self.analysis_pan_drag_start = None;
        self.analysis_guide_drag = None;
        self.analysis_overlay_cache = None;
        self.analysis_hist_cache = None;
        self.analysis_sv_cache = None;
        self.fs_zoom = 1.0;
        self.fs_pan = egui::Vec2::ZERO;
        self.fs_pan_drag_start = None;
        self.fs_free_rotation = 0.0;
        self.fs_rotation_drag_start = None;

        match self.items.get(idx) {
            Some(GridItem::Image(_))
            | Some(GridItem::ZipImage { .. })
            | Some(GridItem::PdfPage { .. }) => {
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
            Some(GridItem::PdfPage { pdf_path, .. }) => pdf_path.clone(),
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
    fn apply_duplicate_filters(
        &mut self,
        folders: &mut Vec<GridItem>,
        folder_metas: &mut Vec<Option<(i64, i64)>>,
        all_media: &mut Vec<(PathBuf, bool, i64, i64)>,
    ) {
        if self.settings.skip_zip_if_folder_exists {
            Self::filter_zip_duplicates(folders, folder_metas);
        }
        if self.settings.skip_image_if_video_exists {
            self.filter_video_image_duplicates(all_media);
        }
        if self.settings.skip_duplicate_images {
            Self::filter_image_ext_duplicates(all_media, &self.settings.image_ext_priority);
        }
    }

    /// ZIP + フォルダの重複: 同名フォルダがあれば ZIP エントリをスキップ。
    /// folders と folder_metas は同じ順序で対応しているため、同期して削除する。
    fn filter_zip_duplicates(
        folders: &mut Vec<GridItem>,
        folder_metas: &mut Vec<Option<(i64, i64)>>,
    ) {
        let real_folder_names: std::collections::HashSet<String> = folders
            .iter()
            .filter_map(|item| {
                if let GridItem::Folder(p) = item {
                    return p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.to_lowercase());
                }
                None
            })
            .collect();

        let mut keep = vec![true; folders.len()];
        for (i, item) in folders.iter().enumerate() {
            if let GridItem::ZipFile(p) = item {
                if real_folder_names.contains(&stem_lower(p)) {
                    keep[i] = false;
                }
            }
        }
        let mut ki = keep.iter();
        folders.retain(|_| *ki.next().unwrap());
        let mut ki = keep.iter();
        folder_metas.retain(|_| *ki.next().unwrap());
    }

    /// 動画 + 画像の重複: 同名の動画があれば画像をスキップし、
    /// 画像ファイルを動画のサムネイルソースとして記録する。
    fn filter_video_image_duplicates(
        &mut self,
        all_media: &mut Vec<(PathBuf, bool, i64, i64)>,
    ) {
        let video_stems: std::collections::HashSet<String> = all_media.iter()
            .filter(|(_, is_video, _, _)| *is_video)
            .map(|(p, _, _, _)| stem_lower(p))
            .collect();

        if video_stems.is_empty() { return; }

        for (p, is_video, _, _) in all_media.iter() {
            if *is_video { continue; }
            let stem = stem_lower(p);
            if video_stems.contains(&stem) {
                self.video_thumb_overrides.insert(stem, p.clone());
            }
        }

        all_media.retain(|(p, is_video, _, _)| {
            *is_video || !video_stems.contains(&stem_lower(p))
        });
    }

    /// 同名画像の拡張子重複: 優先度リストに基づいてフィルタ。
    fn filter_image_ext_duplicates(
        all_media: &mut Vec<(PathBuf, bool, i64, i64)>,
        priority: &[String],
    ) {
        // ステム → (最優先の拡張子の優先度, インデックス)
        let mut best: std::collections::HashMap<String, (usize, usize)> =
            std::collections::HashMap::new();

        for (i, (p, is_video, _, _)) in all_media.iter().enumerate() {
            if *is_video { continue; }
            let stem = stem_lower(p);
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            let prio = priority.iter().position(|e| e == &ext).unwrap_or(usize::MAX);
            match best.get(&stem) {
                Some(&(existing_prio, _)) if prio >= existing_prio => {}
                _ => { best.insert(stem, (prio, i)); }
            }
        }

        // 同名ステムの画像が複数あるか判定
        let mut stem_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (p, is_video, _, _) in all_media.iter() {
            if *is_video { continue; }
            *stem_counts.entry(stem_lower(p)).or_insert(0) += 1;
        }

        let keep_indices: std::collections::HashSet<usize> = best.iter()
            .filter(|(stem, _)| stem_counts.get(stem.as_str()).copied().unwrap_or(0) > 1)
            .map(|(_, &(_, idx))| idx)
            .collect();

        if !keep_indices.is_empty() {
            let mut i = 0;
            all_media.retain(|(p, is_video, _, _)| {
                let current_i = i;
                i += 1;
                if *is_video { return true; }
                let stem = stem_lower(p);
                if stem_counts.get(&stem).copied().unwrap_or(0) <= 1 { return true; }
                keep_indices.contains(&current_i)
            });
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
        self.cached_nav_indices = None;
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
    /// 通常画像 / ZIP エントリ / PDF ページ の全てに対応。
    /// GIF / APNG はアニメーションフレームを全デコードして FsLoadResult::Animated を送信する。
    pub(crate) fn start_fs_load(&mut self, idx: usize) {
        // (path, zip_entry, pdf_page, pdf_password) を取り出し
        let (path, zip_entry, pdf_page, pdf_password) = match self.items.get(idx) {
            Some(GridItem::Image(p)) => (p.clone(), None, None, None),
            Some(GridItem::ZipImage { zip_path, entry_name }) => {
                (zip_path.clone(), Some(entry_name.clone()), None, None)
            }
            Some(GridItem::PdfPage { pdf_path, page_num }) => {
                (pdf_path.clone(), None, Some(*page_num), self.pdf_current_password.clone())
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
            let (name, ext) = if let Some(page_num) = pdf_page {
                (format!("Page {}", page_num + 1), "pdf".to_string())
            } else if let Some(ref entry_name) = zip_entry {
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

            // PDF ページの場合はラスタライズ
            if let Some(page_num) = pdf_page {
                match crate::pdf_loader::render_page(&path, page_num, 4096, pdf_password.as_deref(), Some(cancel.clone())) {
                    Ok(img) => {
                        let elapsed = t.elapsed().as_secs_f64() * 1000.0;
                        crate::logger::log(format!(
                            "  fs load pdf: {elapsed:.0}ms  idx={idx}  {name}  {}x{}",
                            img.width(), img.height()
                        ));
                        let ci = dynamic_image_to_color_image(&img);
                        let _ = tx.send(FsLoadResult::Static(ci));
                    }
                    Err(e) => {
                        if cancel.load(Ordering::Relaxed) {
                            crate::logger::log(format!("  fs pdf render cancelled  {name}"));
                        } else {
                            crate::logger::log(format!("  fs pdf render FAIL: {e}  {name}"));
                            let _ = tx.send(FsLoadResult::Failed);
                        }
                    }
                }
                return;
            }

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
                    let (w, h) = (img.width(), img.height());
                    let ci = dynamic_image_to_color_image(&img);
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

    /// PDF ページをズーム倍率に応じた解像度で非同期再レンダリングする。
    ///
    /// ワーカーに直接リクエストを送り、結果は `poll_pdf_rerender` で受け取る。
    /// UI スレッドを一切ブロックしない。
    pub(crate) fn request_pdf_rerender(&mut self, idx: usize, zoom: f32) {
        let (pdf_path, page_num, password) = match self.items.get(idx) {
            Some(GridItem::PdfPage { pdf_path, page_num }) => {
                (pdf_path.clone(), *page_num, self.pdf_current_password.clone())
            }
            _ => return,
        };

        // 上限 8192: これ以上大きいとテクスチャメモリが巨大になりクラッシュする
        // (8192px 正方形 ≈ 256 MB RGBA、16384px ≈ 1 GB)
        let target_px = ((4096.0 * zoom) as u32).clamp(256, 8192);

        // 既に同じ解像度のキャッシュがあれば不要
        if let Some(FsCacheEntry::Static { pixels, .. }) = self.fs_cache.get(&idx) {
            let cached_long = pixels.size[0].max(pixels.size[1]) as u32;
            let ratio = cached_long as f32 / target_px as f32;
            if (0.9..=1.1).contains(&ratio) {
                return;
            }
        }

        // 進行中の再レンダリングがあればキャンセル
        if let Some((cancel, _)) = self.fs_pending.remove(&idx) {
            cancel.store(true, Ordering::Relaxed);
        }

        // ワーカーに非同期リクエスト (UI スレッドをブロックしない)
        let (cancel, render_rx) = crate::pdf_loader::render_page_async(
            &pdf_path, page_num, target_px, password.as_deref(),
        );

        // render_page_async は DynamicImage チャネルを返すが、fs_pending は
        // FsLoadResult チャネルを期待するため、ブリッジスレッドで変換する
        let (fs_tx, fs_rx) = mpsc::channel::<FsLoadResult>();
        self.fs_pending.insert(idx, (Arc::clone(&cancel), fs_rx));

        std::thread::spawn(move || {
            match render_rx.recv() {
                Ok(Ok(img)) => {
                    if cancel.load(Ordering::Relaxed) { return; }
                    crate::logger::log(format!(
                        "  pdf rerender done: page={} target_px={target_px} {}x{}",
                        page_num + 1, img.width(), img.height()
                    ));
                    let ci = dynamic_image_to_color_image(&img);
                    let _ = fs_tx.send(FsLoadResult::Static(ci));
                }
                Ok(Err(e)) => {
                    crate::logger::log(format!("  pdf rerender FAIL: {e}"));
                    let _ = fs_tx.send(FsLoadResult::Failed);
                }
                Err(_) => {
                    crate::logger::log("  pdf rerender: cancelled (channel closed)".to_string());
                    // キャンセル時は fs_tx を drop して poll_prefetch が Disconnected で除去
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
                matches!(item, GridItem::Image(_) | GridItem::ZipImage { .. } | GridItem::PdfPage { .. }).then_some(i)
            })
            .collect()
    }


    /// フルスクリーン表示を終了し、先読みキャッシュを全クリアする。
    pub(crate) fn close_fullscreen(&mut self) {
        self.fullscreen_idx = None;
        self.slideshow_playing = false;
        self.fs_viewport_shown = false;
        self.fs_secondary_press_start = None;
        self.fs_context_menu_idx = None;
        for (cancel, _) in self.fs_pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.fs_pending.clear();
        self.fs_cache.clear();
        // AI キャッシュもクリア
        for (cancel, _) in self.ai_upscale_pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.ai_upscale_pending.clear();
        self.ai_upscale_cache.clear();
        self.ai_classify_cache.clear();
        // Inpaint キャッシュもクリア
        if let Some((cancel, _, _)) = self.ai_inpaint_pending.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.ai_inpaint_cache.clear();
        self.ai_inpaint_failed.clear();
    }

    // -------------------------------------------------------------------
    // AI アップスケール
    // -------------------------------------------------------------------

    /// AI ランタイムとモデルマネージャを遅延初期化する。
    pub(crate) fn ensure_ai_runtime(&mut self) {
        if self.ai_runtime.is_none() {
            match crate::ai::runtime::AiRuntime::new() {
                Ok(rt) => {
                    self.ai_runtime = Some(std::sync::Arc::new(rt));
                    crate::logger::log("[AI] Runtime initialized".to_string());
                }
                Err(e) => {
                    crate::logger::log(format!("[AI] Runtime init failed: {e}"));
                }
            }
        }
        if self.ai_model_manager.is_none() {
            self.ai_model_manager = Some(std::sync::Arc::new(
                crate::ai::model_manager::ModelManager::new(),
            ));
        }
    }

    /// AI アップスケールの完了をポーリングし、テクスチャに変換してキャッシュする。
    pub(crate) fn poll_ai_upscale(&mut self, ctx: &egui::Context) {
        if !self.ai_upscale_enabled {
            return;
        }

        let mut completed: Vec<(usize, crate::ai::upscale::UpscaleResult)> = Vec::new();
        let mut disconnected: Vec<usize> = Vec::new();

        for (&key, (_, rx)) in &self.ai_upscale_pending {
            match rx.try_recv() {
                Ok(result) => completed.push((key, result)),
                Err(mpsc::TryRecvError::Disconnected) => disconnected.push(key),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        for key in disconnected {
            self.ai_upscale_pending.remove(&key);
            // スレッドが結果を送らずに終了 = 失敗。リトライを防止する。
            self.ai_upscale_failed.insert(key);
        }

        let repaint = !completed.is_empty();
        for (key, result) in completed {
            self.ai_upscale_pending.remove(&key);
            let pixels = std::sync::Arc::new(result.image);
            let upload = clamp_for_gpu(&pixels);
            let handle = ctx.load_texture(
                format!("ai_fs_{key}"),
                upload.into_owned(),
                egui::TextureOptions::LINEAR,
            );
            // 色調補正を即座に適用（シャープネス除く）→ adjustment_cache を更新
            self.apply_fast_adjustment(ctx, key, &pixels);
            self.ai_upscale_cache.insert(key, FsCacheEntry::Static {
                tex: handle,
                pixels,
            });
            crate::logger::log(format!("[AI] Upscale complete for idx={key}"));
        }

        if repaint {
            ctx.request_repaint();
        }
    }

    /// 現在のフルスクリーン画像に対して AI アップスケールを開始する。
    /// - 先読みが全完了している場合のみ開始
    /// - すでにアップスケール済み or pending の場合はスキップ
    /// - 2K 以上の画像はスキップ
    pub(crate) fn maybe_start_ai_upscale(&mut self, current_idx: usize) {
        if !self.ai_upscale_enabled {
            return;
        }

        // すでにアップスケール済み、処理中、または失敗済み
        if self.ai_upscale_cache.contains_key(&current_idx)
            || self.ai_upscale_pending.contains_key(&current_idx)
            || self.ai_upscale_failed.contains(&current_idx)
        {
            return;
        }

        // 画像の先読みがまだ完了していない場合はスキップ
        if !self.fs_pending.is_empty() {
            return;
        }

        // 同時実行は 1 枚まで（GPU メモリと帯域の制約）
        if !self.ai_upscale_pending.is_empty() {
            return;
        }

        // 元画像がキャッシュにあるか確認
        let source_image = match self.fs_cache.get(&current_idx) {
            Some(FsCacheEntry::Static { pixels, .. }) => pixels.clone(),
            _ => return,
        };

        // 2K 以上はスキップ
        let (w, h) = (source_image.size[0] as u32, source_image.size[1] as u32);
        if !crate::ai::upscale::should_upscale(w, h) {
            return;
        }

        // AI ランタイム / モデルマネージャを遅延初期化
        self.ensure_ai_runtime();

        let Some(runtime) = self.ai_runtime.clone() else { return; };
        let Some(manager) = self.ai_model_manager.clone() else { return; };

        // モデル選択
        let model_kind = match self.ai_upscale_model_override {
            Some(k) => k,
            None => {
                // 自動判別: キャッシュ済みならそれを使用、なければヒューリスティクス
                let category = self.ai_classify_cache
                    .get(&current_idx)
                    .copied()
                    .unwrap_or_else(|| {
                        // ヒューリスティクスで判別（モデルなしフォールバック）
                        let dynimg = color_image_to_dynamic(&source_image);
                        let cat = crate::ai::classify::classify_heuristic(&dynimg);
                        self.ai_classify_cache.insert(current_idx, cat);
                        cat
                    });
                category.preferred_upscale_model()
            }
        };

        // モデルファイルが存在するか確認
        let Some(model_path) = manager.model_path(model_kind) else {
            crate::logger::log(format!(
                "[AI] Model {:?} not available, skipping upscale for idx={current_idx}",
                model_kind
            ));
            return;
        };

        // モデルをロード（未ロードの場合）
        if !runtime.is_loaded(model_kind) {
            if let Err(e) = runtime.load_model(model_kind, &model_path) {
                crate::logger::log(format!("[AI] Model load failed: {e}"));
                return;
            }
        }

        // バックグラウンドスレッドでアップスケール実行
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let cancel_clone = cancel.clone();
        let idx = current_idx;

        std::thread::spawn(move || {
            // ColorImage → DynamicImage に変換
            let dynimg = color_image_to_dynamic(&source_image);
            match crate::ai::upscale::upscale(&runtime, model_kind, &dynimg, &cancel_clone) {
                Ok(upscaled) => {
                    let _ = tx.send(crate::ai::upscale::UpscaleResult {
                        idx,
                        image: upscaled,
                    });
                }
                Err(e) => {
                    crate::logger::log(format!("[AI] Upscale failed for idx={idx}: {e}"));
                }
            }
        });

        self.ai_upscale_pending.insert(current_idx, (cancel, rx));
        crate::logger::log(format!(
            "[AI] Upscale started for idx={current_idx} with {:?}",
            model_kind
        ));
    }

    /// AI 補完の完了をポーリングし、テクスチャに変換してキャッシュする。
    pub(crate) fn poll_ai_inpaint(&mut self, ctx: &egui::Context) {
        let result = if let Some((_, ref rx, _)) = self.ai_inpaint_pending {
            match rx.try_recv() {
                Ok(r) => Some(Ok(r)),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(())),
                Err(mpsc::TryRecvError::Empty) => None,
            }
        } else {
            None
        };

        match result {
            Some(Ok(inpaint_result)) => {
                self.ai_inpaint_pending = None;
                let key = (
                    inpaint_result.left_idx,
                    inpaint_result.right_idx,
                    inpaint_result.gap_width,
                    inpaint_result.trim,
                );
                crate::logger::log(format!(
                    "[AI] Inpaint complete: {}x{}, key=({},{},{},{})",
                    inpaint_result.combined.size[0], inpaint_result.combined.size[1],
                    key.0, key.1, key.2, key.3
                ));
                let upload = clamp_for_gpu(&inpaint_result.combined);
                let handle = ctx.load_texture(
                    format!("ai_inpaint_{}_{}_{}_{}",
                        key.0, key.1, key.2, key.3),
                    upload.into_owned(),
                    egui::TextureOptions::LINEAR,
                );
                self.ai_inpaint_cache.insert(key, handle);
                ctx.request_repaint();
            }
            Some(Err(())) => {
                // Disconnected — スレッドが送信せずに終了（エラー）
                let failed_key = self.ai_inpaint_pending.as_ref().map(|(_, _, k)| *k);
                crate::logger::log(format!("[AI] Inpaint failed (worker disconnected): key={:?}", failed_key));
                if let Some(key) = failed_key {
                    self.ai_inpaint_failed.insert(key);
                }
                self.ai_inpaint_pending = None;
            }
            None => {}
        }
    }

    /// 見開き中央の AI 補完を開始する。
    pub(crate) fn start_ai_inpaint(
        &mut self,
        left_idx: usize,
        right_idx: usize,
        gap_width: u32,
        trim: u32,
    ) {
        crate::logger::log(format!(
            "[AI] start_ai_inpaint called: left={left_idx}, right={right_idx}, gap={gap_width}, trim={trim}"
        ));

        // 既存の pending をキャンセル
        if let Some((cancel, _, _)) = self.ai_inpaint_pending.take() {
            cancel.store(true, Ordering::Relaxed);
            crate::logger::log("[AI] Cancelled previous inpaint job".to_string());
        }

        let key = (left_idx, right_idx, gap_width, trim);

        // すでにキャッシュにあればスキップ
        if self.ai_inpaint_cache.contains_key(&key) {
            crate::logger::log("[AI] Inpaint cache hit, skipping".to_string());
            return;
        }

        // 以前失敗したキーはスキップ
        if self.ai_inpaint_failed.contains(&key) {
            return;
        }

        // 左右の画像を取得
        let left_pixels = match self.fs_cache.get(&left_idx) {
            Some(FsCacheEntry::Static { pixels, .. }) => pixels.clone(),
            _ => {
                crate::logger::log(format!("[AI] Inpaint: left image not in fs_cache (idx={left_idx})"));
                return;
            }
        };
        let right_pixels = match self.fs_cache.get(&right_idx) {
            Some(FsCacheEntry::Static { pixels, .. }) => pixels.clone(),
            _ => {
                crate::logger::log(format!("[AI] Inpaint: right image not in fs_cache (idx={right_idx})"));
                return;
            }
        };

        // AI ランタイム / モデルマネージャを遅延初期化
        self.ensure_ai_runtime();

        let Some(runtime) = self.ai_runtime.clone() else {
            crate::logger::log("[AI] Inpaint: ai_runtime not available".to_string());
            return;
        };
        let Some(manager) = self.ai_model_manager.clone() else {
            crate::logger::log("[AI] Inpaint: ai_model_manager not available".to_string());
            return;
        };

        // MI-GAN モデルが利用可能か確認、なければ自動ダウンロード開始
        let Some(model_path) = manager.model_path(crate::ai::ModelKind::InpaintMiGan) else {
            crate::logger::log("[AI] MI-GAN model not available, starting download...".to_string());
            manager.start_download(crate::ai::ModelKind::InpaintMiGan);
            return;
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = cancel.clone();
        let (tx, rx) = mpsc::channel();

        // モデルロード + 推論を全てバックグラウンドスレッドで実行
        std::thread::spawn(move || {
            // モデルがまだロードされていなければロード（MI-GAN は DirectML 対応）
            if !runtime.is_loaded(crate::ai::ModelKind::InpaintMiGan) {
                crate::logger::log("[AI] Loading MI-GAN model in background...".to_string());
                if let Err(e) = runtime.load_model(crate::ai::ModelKind::InpaintMiGan, &model_path) {
                    crate::logger::log(format!("[AI] MI-GAN model load failed: {e}"));
                    return;
                }
                crate::logger::log("[AI] MI-GAN model loaded".to_string());
            }

            if cancel_clone.load(Ordering::Relaxed) {
                return;
            }

            match crate::ai::inpaint::inpaint_spread(
                &runtime,
                &left_pixels,
                &right_pixels,
                gap_width,
                trim,
                &cancel_clone,
            ) {
                Ok(combined) => {
                    let _ = tx.send(crate::ai::inpaint::InpaintResult {
                        left_idx,
                        right_idx,
                        gap_width,
                        trim,
                        combined,
                    });
                }
                Err(e) => {
                    crate::logger::log(format!("[AI] Inpaint failed: {e}"));
                }
            }
        });

        self.ai_inpaint_pending = Some((cancel, rx, key));
        crate::logger::log(format!(
            "[AI] Inpaint started: left={left_idx}, right={right_idx}, gap={gap_width}"
        ));
    }

    /// 先読み範囲内の item_idx 集合を計算する。
    fn compute_keep_set(&self, current_idx: usize) -> std::collections::HashSet<usize> {
        let image_indices = Self::collect_image_indices(&self.items);
        let Some(pos) = image_indices.iter().position(|&i| i == current_idx) else {
            return std::collections::HashSet::new();
        };
        let n = image_indices.len();
        let keep_back = self.settings.prefetch_back + 1;
        let keep_forward = self.settings.prefetch_forward + 1;
        (pos.saturating_sub(keep_back)..=((pos + keep_forward).min(n - 1)))
            .map(|p| image_indices[p])
            .collect()
    }

    /// AI アップスケールキャッシュの eviction（先読み範囲外を破棄）。
    fn evict_ai_upscale_cache(&mut self, current_idx: usize) {
        let keep_set = self.compute_keep_set(current_idx);
        self.ai_upscale_cache.retain(|k, _| keep_set.contains(k));

        // 範囲外の pending をキャンセル
        let to_cancel: Vec<usize> = self.ai_upscale_pending.keys()
            .filter(|k| !keep_set.contains(k))
            .cloned()
            .collect();
        for k in to_cancel {
            if let Some((cancel, _)) = self.ai_upscale_pending.remove(&k) {
                cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    /// AI アップスケールの先読み（表示中画像の前後）。
    fn prefetch_ai_upscale(&mut self, current_idx: usize) {
        if !self.ai_upscale_enabled {
            return;
        }

        let image_indices = Self::collect_image_indices(&self.items);
        let Some(pos) = image_indices.iter().position(|&i| i == current_idx) else { return; };
        let n = image_indices.len();

        let pf_back = self.settings.ai_upscale_prefetch_back;
        let pf_forward = self.settings.ai_upscale_prefetch_forward;

        // 前方優先（+1, +2, … , -1, -2, …）
        let targets: Vec<usize> = (1..=pf_forward)
            .filter_map(|d| pos.checked_add(d).filter(|&p| p < n).map(|p| image_indices[p]))
            .chain(
                (1..=pf_back)
                    .filter_map(|d| pos.checked_sub(d).map(|p| image_indices[p]))
            )
            .collect();

        for idx in targets {
            self.maybe_start_ai_upscale(idx);
        }
    }

    // ── 画像補正 ──────────────────────────────────────────────────

    /// ページの正規化キーを返す（DB 保存用）。
    pub(crate) fn page_path_key(&self, idx: usize) -> Option<String> {
        let item = self.items.get(idx)?;
        let key = match item {
            GridItem::Image(p) => crate::adjustment_db::normalize_path(p),
            GridItem::ZipImage { zip_path, entry_name } => {
                format!("{}::{}", crate::adjustment_db::normalize_path(zip_path), entry_name.to_lowercase())
            }
            GridItem::PdfPage { pdf_path, page_num } => {
                format!("{}::page_{}", crate::adjustment_db::normalize_path(pdf_path), page_num)
            }
            _ => return None,
        };
        Some(key)
    }

    /// 現在のプリセット設定に基づいて AI アップスケールの有効/モデルを更新する。
    pub(crate) fn sync_upscale_from_preset(&mut self, idx: usize) {
        let preset_idx = self.adjustment_page_preset.get(&idx).copied()
            .or(self.adjustment_active_preset);
        if let Some(pi) = preset_idx {
            let params = &self.adjustment_presets.presets[pi as usize];
            match params.upscale_model_kind() {
                None => {
                    self.ai_upscale_enabled = false;
                    self.ai_upscale_model_override = None;
                }
                Some(None) => {
                    self.ai_upscale_enabled = true;
                    self.ai_upscale_model_override = None;
                }
                Some(Some(kind)) => {
                    self.ai_upscale_enabled = true;
                    self.ai_upscale_model_override = Some(kind);
                }
            }
        } else {
            self.ai_upscale_enabled = false;
            self.ai_upscale_model_override = None;
        }
    }

    /// 補正バックグラウンド処理の結果をポーリングする。
    pub(crate) fn poll_adjustment(&mut self, ctx: &egui::Context) {
        if let Some((idx, ref rx)) = self.adjustment_pending {
            match rx.try_recv() {
                Ok(color_image) => {
                    let pixels = std::sync::Arc::new(color_image);
                    let upload = clamp_for_gpu(&pixels);
                    let tex = ctx.load_texture(
                        format!("adj_{idx}"),
                        upload.into_owned(),
                        egui::TextureOptions::LINEAR,
                    );
                    self.adjustment_cache.insert(idx, FsCacheEntry::Static { tex, pixels });
                    self.adjustment_sharpened.insert(idx);
                    self.adjustment_pending = None;
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.adjustment_pending = None;
                }
            }
        }
    }

    /// 指定 idx に対応するプリセットパラメータを取得する。
    fn get_adjustment_params(&self, idx: usize) -> Option<crate::adjustment::AdjustParams> {
        let pi = self.adjustment_page_preset.get(&idx).copied()
            .or(self.adjustment_active_preset)?;
        Some(self.adjustment_presets.presets[pi as usize].clone())
    }

    /// 画像に即座に色調補正（シャープネス除く）を適用して adjustment_cache に格納する。
    /// poll_prefetch / poll_ai_upscale から呼ばれる。
    pub(crate) fn apply_fast_adjustment(&mut self, ctx: &egui::Context, idx: usize, pixels: &std::sync::Arc<egui::ColorImage>) {
        let Some(params) = self.get_adjustment_params(idx) else { return; };
        if params.is_identity() && !crate::adjustment::needs_sharpen(&params) {
            return; // 補正不要
        }
        if params.is_identity() {
            // 色調補正なし、シャープネスのみ → バックグラウンドで処理
            // adjustment_cache にはソースをそのまま入れない（後で sharpen が更新する）
            return;
        }
        let adjusted = crate::adjustment::apply_adjustments_fast(pixels, &params);
        let adjusted_pixels = std::sync::Arc::new(adjusted);
        let upload = clamp_for_gpu(&adjusted_pixels);
        let tex = ctx.load_texture(
            format!("adj_{idx}"),
            upload.into_owned(),
            egui::TextureOptions::LINEAR,
        );
        self.adjustment_cache.insert(idx, FsCacheEntry::Static { tex, pixels: adjusted_pixels });
    }

    /// スライダードラッグ中の低解像度プレビューを生成する。
    /// パラメータが前回と同じなら再処理をスキップする。
    pub(crate) fn update_adjustment_preview(&mut self, ctx: &egui::Context, idx: usize) {
        if !self.adjustment_dragging {
            return;
        }
        let Some(params) = self.get_adjustment_params(idx) else { return; };
        if params.is_identity() {
            return;
        }

        // パラメータが前回と同じならスキップ
        if self.adjustment_preview_params.as_ref() == Some(&params) {
            return;
        }

        let source = self.fs_cache.get(&idx);
        let Some(FsCacheEntry::Static { pixels, .. }) = source else { return; };

        let preview = crate::adjustment::apply_adjustments_preview(pixels, &params);
        let tex = ctx.load_texture(
            "adj_preview",
            preview,
            egui::TextureOptions::LINEAR,
        );
        self.adjustment_preview_tex = Some(tex);
        self.adjustment_preview_params = Some(params);
        ctx.request_repaint();
    }

    /// シャープネス処理をバックグラウンドで開始する。
    /// 色調補正は poll_prefetch / poll_ai_upscale で同期適用済み。
    /// シャープネスだけ重い（畳み込み）ためバックグラウンド処理。
    pub(crate) fn maybe_start_adjustment(&mut self, idx: usize) {
        // 既にシャープネス適用済み or 処理中なら何もしない
        if self.adjustment_sharpened.contains(&idx) {
            return;
        }
        if self.adjustment_pending.as_ref().map_or(false, |(i, _)| *i == idx) {
            return;
        }

        let Some(params) = self.get_adjustment_params(idx) else { return; };
        if !crate::adjustment::needs_sharpen(&params) {
            return; // シャープネス不要
        }

        // ソース画像を取得（adjustment_cache に色調補正済みがあればそちら、
        // なければ ai_upscale_cache or fs_cache）
        let source = self.adjustment_cache.get(&idx)
            .or_else(|| if self.ai_upscale_enabled { self.ai_upscale_cache.get(&idx) } else { None })
            .or_else(|| self.fs_cache.get(&idx));
        let Some(FsCacheEntry::Static { pixels, .. }) = source else { return; };

        // 既にシャープネス適用済みかチェック（adjustment_cache にあり、
        // かつ色調補正済みソースのサイズと一致 = まだシャープネス未適用の可能性）
        // → pending がなければシャープネスを適用
        let pixels = std::sync::Arc::clone(pixels);

        let (tx, rx) = mpsc::channel();
        self.adjustment_pending = Some((idx, rx));
        std::thread::spawn(move || {
            let result = crate::adjustment::apply_sharpen_only(&pixels, &params);
            let _ = tx.send(result);
        });
    }

    /// 指定ページの補正関連キャッシュをすべてクリアする。
    /// プリセット切替・パラメータ変更時に呼ぶ。
    pub(crate) fn clear_adjustment_caches(&mut self, idx: usize) {
        self.adjustment_cache.remove(&idx);
        self.adjustment_sharpened.remove(&idx);
        self.adjustment_preview_tex = None;
        self.adjustment_preview_params = None;
        self.ai_upscale_cache.clear();
        self.ai_upscale_failed.clear();
        for (_, (cancel, _)) in self.ai_upscale_pending.drain() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// 補正キャッシュを evict する（prefetch 範囲外）。
    pub(crate) fn evict_adjustment_cache(&mut self, current_idx: usize) {
        let keep_set = self.compute_keep_set(current_idx);
        self.adjustment_cache.retain(|k, _| keep_set.contains(k));
        self.adjustment_sharpened.retain(|k| keep_set.contains(k));
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
    pub(crate) fn poll_prefetch(&mut self, ctx: &egui::Context) {
        let mut completed: Vec<(usize, FsLoadResult)> = Vec::new();
        let mut disconnected: Vec<usize> = Vec::new();
        for (&key, (_, rx)) in &self.fs_pending {
            match rx.try_recv() {
                Ok(result) => completed.push((key, result)),
                Err(mpsc::TryRecvError::Disconnected) => disconnected.push(key),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        // 送信側が drop されたエントリを除去 (キャンセル済みスレッドが送信せずに終了)
        for key in disconnected {
            self.fs_pending.remove(&key);
        }
        let repaint = !completed.is_empty();
        for (key, result) in completed {
            self.fs_pending.remove(&key);
            let entry = match result {
                FsLoadResult::Static(ci) => {
                    let pixels = std::sync::Arc::new(ci);
                    let upload = clamp_for_gpu(&pixels);
                    let handle = ctx.load_texture(
                        format!("fs_{key}"),
                        upload.into_owned(),
                        egui::TextureOptions::LINEAR,
                    );
                    // 色調補正を即座に適用（シャープネス除く）
                    self.apply_fast_adjustment(ctx, key, &pixels);
                    FsCacheEntry::Static { tex: handle, pixels }
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
        // 選択されたお気に入りを集める（名前とパスのペア）
        let targets: Vec<(String, PathBuf)> = self
            .settings
            .favorites
            .iter()
            .zip(self.cc.checked.iter())
            .filter_map(|(f, &c)| if c { Some((f.name.clone(), f.path.clone())) } else { None })
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
        let batch_zip = self.settings.batch_cache_zip_contents;
        let batch_pdf = self.settings.batch_cache_pdf_contents;

        std::thread::spawn(move || {
            // Pass 1: カウント
            let mut all_folders: Vec<PathBuf> = Vec::new();
            for (_, path) in &targets {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                walk_dirs_recursive(path, &mut all_folders, &cancel);
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

                // お気に入り名 > 相対パス の形式で表示用文字列を生成
                let folder_display = targets.iter()
                    .find(|(_, base)| folder.starts_with(base))
                    .map(|(name, base)| {
                        match folder.strip_prefix(base) {
                            Ok(rel) if rel.as_os_str().is_empty() => name.clone(),
                            Ok(rel) => format!("{} > {}", name, rel.to_string_lossy()),
                            Err(_) => folder.to_string_lossy().to_string(),
                        }
                    })
                    .unwrap_or_else(|| folder.to_string_lossy().to_string());
                *current.lock().unwrap() = folder_display.clone();

                // ファイル列挙（単一フォルダ、再帰なし — 画像・ZIP・PDF を1パスで分類）
                let mut images: Vec<(PathBuf, i64, i64)> = Vec::new();
                let mut zip_files: Vec<(PathBuf, i64, i64)> = Vec::new();
                let mut pdf_files: Vec<(PathBuf, i64, i64)> = Vec::new();
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
                        let meta = || {
                            let m = entry.metadata().ok()?;
                            let mtime = crate::ui_helpers::mtime_secs(&m);
                            let file_size = m.len() as i64;
                            Some((mtime, file_size))
                        };
                        if SUPPORTED_EXTENSIONS.contains(&ext_lower.as_str()) {
                            if let Some((mt, fs)) = meta() {
                                images.push((p, mt, fs));
                            }
                        } else if ext_lower == "zip" {
                            if let Some((mt, fs)) = meta() {
                                zip_files.push((p, mt, fs));
                            }
                        } else if ext_lower == "pdf" {
                            if let Some((mt, fs)) = meta() {
                                pdf_files.push((p, mt, fs));
                            }
                        }
                    }
                }

                if images.is_empty() && zip_files.is_empty() && pdf_files.is_empty() {
                    done.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                // カタログを開く（1フォルダ1DB）
                let Ok(catalog) = crate::catalog::CatalogDb::open(&cache_dir, folder) else {
                    done.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let cache_map = catalog.load_all().unwrap_or_default();

                // ── 画像を並列でデコード + 保存 ──
                if !images.is_empty() {
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
                }

                // ── ZIP ファイルの中身をキャッシュ ──
                for (zip_path, zip_mtime, zip_file_size) in &zip_files {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let zip_fname = match zip_path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    let folder_key = format!("{}{}", CACHE_KEY_ZIP, zip_fname);

                    if batch_zip {
                        *current.lock().unwrap() = format!("{} > {}", folder_display, zip_fname);
                        let entries = match crate::zip_loader::enumerate_image_entries(zip_path) {
                            Ok(e) => e,
                            Err(_) => continue,
                        };
                        let zip_catalog = match crate::catalog::CatalogDb::open(&cache_dir, zip_path) {
                            Ok(c) => c,
                            Err(_) => continue,
                        };
                        let zip_cache_map = zip_catalog.load_all().unwrap_or_default();
                        let entry_count = entries.len();

                        // 先頭エントリの WebP を並列処理中にキャプチャ
                        let first_webp: Arc<Mutex<Option<(image::DynamicImage, String)>>> =
                            Arc::new(Mutex::new(None));

                        pool.install(|| {
                            use rayon::prelude::*;
                            entries.par_iter().enumerate().for_each(|(i, entry)| {
                                if cancel.load(Ordering::Relaxed) {
                                    return;
                                }
                                *current.lock().unwrap() = format!(
                                    "{} > {} ({}/{})", folder_display, zip_fname, i + 1, entry_count
                                );
                                if let Some(existing) = zip_cache_map.get(&entry.entry_name) {
                                    if existing.mtime == entry.mtime
                                        && existing.file_size == entry.uncompressed_size as i64
                                    {
                                        return;
                                    }
                                }
                                let raw = match crate::zip_loader::read_entry_bytes(zip_path, &entry.entry_name) {
                                    Ok(b) => b,
                                    Err(_) => return,
                                };
                                let img = match image::load_from_memory(&raw) {
                                    Ok(i) => i,
                                    Err(_) => return,
                                };
                                // 先頭エントリをキャプチャ（親フォルダ用サムネイル再利用）
                                if i == 0 {
                                    *first_webp.lock().unwrap() = Some((img.clone(), entry.entry_name.clone()));
                                }
                                if let Some(bytes) = encode_and_save(
                                    &img,
                                    &entry.entry_name,
                                    &zip_catalog,
                                    entry.mtime,
                                    entry.uncompressed_size as i64,
                                    thumb_px,
                                    thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            });
                        });

                        // 先頭1枚を親フォルダの DB にも保存（フォルダ一覧用サムネイル）
                        if !cache_map.contains_key(&folder_key) {
                            let captured = first_webp.lock().unwrap().take();
                            if let Some((img, _)) = captured {
                                if let Some(bytes) = encode_and_save(
                                    &img, &folder_key, &catalog,
                                    *zip_mtime, *zip_file_size, thumb_px, thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            }
                        }
                    } else {
                        // 先頭1枚のみ（フォルダ一覧用サムネイル）
                        if cache_map.contains_key(&folder_key) {
                            continue;
                        }
                        if let Some(first_entry) = crate::zip_loader::first_image_entry(zip_path) {
                            if let Ok(raw) = crate::zip_loader::read_entry_bytes(zip_path, &first_entry) {
                                if let Ok(img) = image::load_from_memory(&raw) {
                                    if let Some(bytes) = encode_and_save(
                                        &img, &folder_key, &catalog,
                                        *zip_mtime, *zip_file_size, thumb_px, thumb_quality,
                                    ) {
                                        size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                }

                // ── PDF ファイルの中身をキャッシュ ──
                if !pdf_files.is_empty() && !cancel.load(Ordering::Relaxed) {
                    let pw_store = crate::pdf_passwords::PdfPasswordStore::load();

                    for (pdf_path, pdf_mtime, pdf_file_size) in &pdf_files {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let pdf_fname = match pdf_path.file_name().and_then(|n| n.to_str()) {
                            Some(n) => n.to_string(),
                            None => continue,
                        };
                        *current.lock().unwrap() = format!("{} > {}", folder_display, pdf_fname);
                        let password = pw_store.get(pdf_path);
                        let pw_ref = password.as_deref();
                        let folder_key = format!("{}{}", CACHE_KEY_PDF, pdf_fname);

                        if batch_pdf {
                            // enumerate_pages がパスワード不正時に Err を返すので
                            // check_password_needed は不要
                            let pages = match crate::pdf_loader::enumerate_pages(pdf_path, pw_ref) {
                                Ok(p) => p,
                                Err(_) => continue,
                            };
                            let pdf_catalog = match crate::catalog::CatalogDb::open(&cache_dir, pdf_path) {
                                Ok(c) => c,
                                Err(_) => continue,
                            };
                            let pdf_cache_map = pdf_catalog.load_all().unwrap_or_default();
                            let page_count = pages.len();

                            // PDFium ワーカーはシングルスレッド → 順次処理
                            for i in 0..page_count {
                                if cancel.load(Ordering::Relaxed) {
                                    break;
                                }
                                let page_num = i as u32;
                                *current.lock().unwrap() = format!(
                                    "{} > {} ({}/{})", folder_display, pdf_fname, i + 1, page_count
                                );
                                let key = crate::grid_item::pdf_page_cache_key(page_num);
                                if let Some(existing) = pdf_cache_map.get(&key) {
                                    if existing.mtime == *pdf_mtime
                                        && existing.file_size == *pdf_file_size
                                    {
                                        continue;
                                    }
                                }
                                if let Some(bytes) = crate::thumb_loader::build_and_save_one_pdf(
                                    pdf_path, page_num, pw_ref, &pdf_catalog,
                                    *pdf_mtime, *pdf_file_size, thumb_px, thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            }

                            // 先頭1ページを親フォルダの DB にも保存
                            if page_count > 0 && !cache_map.contains_key(&folder_key) {
                                if let Ok(img) = crate::pdf_loader::render_page(pdf_path, 0, thumb_px, pw_ref, None) {
                                    if let Some(bytes) = encode_and_save(
                                        &img, &folder_key, &catalog,
                                        *pdf_mtime, *pdf_file_size, thumb_px, thumb_quality,
                                    ) {
                                        size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                    }
                                }
                            }
                        } else {
                            // 先頭1ページのみ（フォルダ一覧用サムネイル）
                            if cache_map.contains_key(&folder_key) {
                                continue;
                            }
                            // render_page がパスワード不正時に Err を返すのでそのままスキップ
                            if let Ok(img) = crate::pdf_loader::render_page(pdf_path, 0, thumb_px, pw_ref, None) {
                                if let Some(bytes) = encode_and_save(
                                    &img, &folder_key, &catalog,
                                    *pdf_mtime, *pdf_file_size, thumb_px, thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                }

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

        self.track_window_rect(ctx);

        // 毎フレームリセット: 選択セルが描画された時に再設定される
        self.selected_cell_rect = None;

        let frame_t0 = std::time::Instant::now();

        self.poll_thumbnails(ctx);
        let t_poll = frame_t0.elapsed();

        self.update_keep_range_and_requests(frame_t0);
        let t_keep = frame_t0.elapsed();

        self.poll_prefetch(ctx);
        self.poll_ai_upscale(ctx);
        self.poll_ai_inpaint(ctx);
        self.poll_adjustment(ctx);

        // AI モデルダウンロード中ならポーリング
        if let Some(ref mgr) = self.ai_model_manager {
            if mgr.has_active_downloads() {
                mgr.poll_downloads();
            }
        }

        // フルスクリーン表示中なら AI アップスケール + 画像補正を検討
        if let Some(fs_idx) = self.fullscreen_idx {
            // プリセットに基づいてアップスケール設定を同期
            self.sync_upscale_from_preset(fs_idx);

            // 表示中画像を最優先でアップスケール
            self.maybe_start_ai_upscale(fs_idx);
            // 表示中画像のアップスケールが完了 or 不要なら先読みもアップスケール
            let current_done = self.ai_upscale_cache.contains_key(&fs_idx)
                || self.ai_upscale_failed.contains(&fs_idx)
                || !self.ai_upscale_enabled
                || self.fs_cache.get(&fs_idx).map(|e| {
                    if let FsCacheEntry::Static { pixels, .. } = e {
                        !crate::ai::upscale::should_upscale(pixels.size[0] as u32, pixels.size[1] as u32)
                    } else { true }
                }).unwrap_or(true);
            if current_done && self.ai_upscale_pending.is_empty() {
                self.prefetch_ai_upscale(fs_idx);
            }
            self.evict_ai_upscale_cache(fs_idx);

            // 画像補正の適用（アップスケール後に適用）
            self.update_adjustment_preview(ctx, fs_idx);
            self.maybe_start_adjustment(fs_idx);
            self.evict_adjustment_cache(fs_idx);
        }

        // タイトルバーに現在のフォルダパスを表示する。
        // フォルダ未選択時や読み込み途中はアプリ名のみ。
        let title = match self.current_folder.as_ref() {
            Some(p) => format!("{} - mimageviewer", p.display()),
            None => "mimageviewer".to_string(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        // スクロールは egui に触れる前に処理（イベントを消費）
        self.process_scroll(ctx);

        self.handle_clipboard_shortcuts(ctx);

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
        self.show_stats_dialog_window(ctx);
        self.show_rotation_reset_confirm_dialog(ctx);
        let context_nav = self.show_context_menu(ctx);
        self.show_delete_confirm_dialog(ctx);
        self.show_pdf_password_dialog_window(ctx);
        self.show_ai_model_setup_dialog(ctx);
        self.show_about_dialog_window(ctx);
        self.poll_pdf_enumerate();

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

        // ── Alt+1〜0: 列数切り替え ──────────────────────────────────
        if !self.address_has_focus && self.fullscreen_idx.is_none() && !self.any_dialog_open() {
            let alt_col = ctx.input(|i| {
                if !i.modifiers.alt { return None; }
                let keys = [
                    (egui::Key::Num1, 1), (egui::Key::Num2, 2), (egui::Key::Num3, 3),
                    (egui::Key::Num4, 4), (egui::Key::Num5, 5), (egui::Key::Num6, 6),
                    (egui::Key::Num7, 7), (egui::Key::Num8, 8), (egui::Key::Num9, 9),
                    (egui::Key::Num0, 10),
                ];
                keys.iter().find(|(k, _)| i.key_pressed(*k)).map(|&(_, c)| c)
            });
            if let Some(cols) = alt_col {
                if cols != self.settings.grid_cols {
                    self.settings.grid_cols = cols;
                    self.settings.save();
                }
            }
        }

        // ── 検索バー ─────────────────────────────────────────────────
        self.render_search_bar(ctx);

        // ── サムネイルグリッド ────────────────────────────────────────
        let t_pre_grid = frame_t0.elapsed();
        let grid_nav = self.render_grid(ctx);
        let t_grid = frame_t0.elapsed();

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

        // ── 非同期フォルダナビゲーションのポーリング ────────────────
        let folder_nav = self.poll_folder_nav();

        // ── ナビゲーション集約 ───────────────────────────────────────
        let navigate = fav_nav
            .or(toolbar_fav_nav)
            .or(keyboard_nav)
            .or(folder_nav)
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
        if self.folder_nav_pending.is_some()
            || self.thumbnails.iter().any(|t| matches!(t, ThumbnailState::Pending))
            || self.pdf_enumerate_pending.is_some()
        {
            ctx.request_repaint();
        }

        // フレーム計測: 8 ms (≈120 fps) 超えた場合のみログに出力
        let frame_total = frame_t0.elapsed();
        if frame_total.as_millis() > 8 {
            crate::logger::log(format!(
                "  [SLOW FRAME] {:.1}ms  poll={:.1}ms keep={:.1}ms pre_grid={:.1}ms grid={:.1}ms  backlog={} requested={}",
                frame_total.as_secs_f64() * 1000.0,
                t_poll.as_secs_f64() * 1000.0,
                (t_keep - t_poll).as_secs_f64() * 1000.0,
                (t_pre_grid - t_keep).as_secs_f64() * 1000.0,
                (t_grid - t_pre_grid).as_secs_f64() * 1000.0,
                self.texture_backlog.len(),
                self.requested.len(),
            ));
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

/// GridItem から LoadRequest を構築する。画像 / ZIP 内画像 / PDF ページ / フォルダ以外は None を返す。
fn make_load_request(
    item: &GridItem,
    idx: usize,
    mtime: i64,
    file_size: i64,
    skip_cache: bool,
    pdf_password: Option<&str>,
    folder_thumb_sort: Option<crate::settings::SortOrder>,
    folder_thumb_depth: u32,
) -> Option<LoadRequest> {
    match item {
        GridItem::Image(p) => Some(LoadRequest {
            idx, path: p.clone(), mtime, file_size,
            skip_cache, priority: false, zip_entry: None, pdf_page: None, pdf_password: None,
            cache_key_override: None, folder_thumb_sort: None, folder_thumb_depth: 0,
        }),
        GridItem::ZipImage { zip_path, entry_name } => Some(LoadRequest {
            idx, path: zip_path.clone(), mtime, file_size,
            skip_cache, priority: false, zip_entry: Some(entry_name.clone()), pdf_page: None, pdf_password: None,
            cache_key_override: None, folder_thumb_sort: None, folder_thumb_depth: 0,
        }),
        GridItem::PdfPage { pdf_path, page_num } => Some(LoadRequest {
            idx, path: pdf_path.clone(), mtime, file_size,
            skip_cache, priority: false, zip_entry: None, pdf_page: Some(*page_num),
            pdf_password: pdf_password.map(String::from),
            cache_key_override: None, folder_thumb_sort: None, folder_thumb_depth: 0,
        }),
        GridItem::ZipFile(p) => {
            // フォルダ一覧用: ZIP の最初の画像エントリをサムネイルとして取得。
            // zip_entry は None のままにしておき、ワーカー側でキャッシュミス時に
            // 遅延解決する。UI スレッドで ZIP を開くディスク I/O を避けるため。
            let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            Some(LoadRequest {
                idx, path: p.clone(), mtime, file_size,
                skip_cache, priority: false, zip_entry: None, pdf_page: None, pdf_password: None,
                cache_key_override: Some(format!("{}{fname}", CACHE_KEY_ZIP)),
                folder_thumb_sort: None, folder_thumb_depth: 0,
            })
        }
        GridItem::PdfFile(p) => {
            // フォルダ一覧用: PDF の 1 ページ目をサムネイルとして取得
            let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            Some(LoadRequest {
                idx, path: p.clone(), mtime, file_size,
                skip_cache, priority: false, zip_entry: None, pdf_page: Some(0),
                pdf_password: pdf_password.map(String::from),
                cache_key_override: Some(format!("{}{fname}", CACHE_KEY_PDF)),
                folder_thumb_sort: None, folder_thumb_depth: 0,
            })
        }
        GridItem::Folder(p) => {
            // フォルダ一覧用: フォルダ内の代表画像をサムネイルとして取得
            let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            Some(LoadRequest {
                idx, path: p.clone(), mtime, file_size,
                skip_cache, priority: false, zip_entry: None, pdf_page: None, pdf_password: None,
                cache_key_override: Some(format!("{}{fname}", CACHE_KEY_FOLDER)),
                folder_thumb_sort, folder_thumb_depth,
            })
        }
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

/// DynamicImage を egui::ColorImage に変換する (リサイズなし)。
/// フルスクリーン表示や PDF 再レンダリング結果の変換で使用。
pub(crate) fn dynamic_image_to_color_image(img: &image::DynamicImage) -> egui::ColorImage {
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

/// wgpu テクスチャの最大次元。wgpu デフォルト制限は 8192px。
/// GPU 実機はもっと大きいが (RTX 4090 = 16384)、eframe が
/// デフォルト Limits で初期化するため 8192 を超えるとパニックする。
const MAX_TEXTURE_DIM: usize = 8192;

/// GPU テクスチャ上限を超える `ColorImage` を縮小して返す。
/// 上限内であればクローンせず共有参照をそのまま `Cow::Borrowed` で返す。
pub(crate) fn clamp_for_gpu(ci: &egui::ColorImage) -> std::borrow::Cow<'_, egui::ColorImage> {
    let [w, h] = ci.size;
    if w <= MAX_TEXTURE_DIM && h <= MAX_TEXTURE_DIM {
        return std::borrow::Cow::Borrowed(ci);
    }
    // 長辺を MAX_TEXTURE_DIM に収めるスケール
    let scale = MAX_TEXTURE_DIM as f64 / w.max(h) as f64;
    let new_w = ((w as f64 * scale).round() as u32).max(1);
    let new_h = ((h as f64 * scale).round() as u32).max(1);
    let dynimg = color_image_to_dynamic(ci);
    let resized = dynimg.resize_exact(new_w, new_h, image::imageops::FilterType::Triangle);
    crate::logger::log(format!(
        "  clamp_for_gpu: {w}x{h} → {new_w}x{new_h} (limit {MAX_TEXTURE_DIM})"
    ));
    std::borrow::Cow::Owned(dynamic_image_to_color_image(&resized))
}

/// 回転した画像を Mesh で描画する。
/// `free_rotation_rad` が非ゼロの場合、`center` 基準で頂点を任意角度回転する。
pub(crate) fn draw_rotated_image_ex(
    painter: &egui::Painter,
    texture_id: egui::TextureId,
    rect: egui::Rect,
    rotation: crate::rotation_db::Rotation,
    free_rotation_rad: f32,
    center: egui::Pos2,
) {
    // UV 座標を回転に合わせて変換
    // 頂点順: 左上, 右上, 右下, 左下 (画面座標)
    let uvs = match rotation {
        crate::rotation_db::Rotation::None => [
            egui::pos2(0.0, 0.0),
            egui::pos2(1.0, 0.0),
            egui::pos2(1.0, 1.0),
            egui::pos2(0.0, 1.0),
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

    let mut positions = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];

    // 任意角度回転: 頂点を center 基準で回転
    if free_rotation_rad.abs() > 0.001 {
        let cos_r = free_rotation_rad.cos();
        let sin_r = free_rotation_rad.sin();
        for p in &mut positions {
            let dx = p.x - center.x;
            let dy = p.y - center.y;
            p.x = center.x + dx * cos_r - dy * sin_r;
            p.y = center.y + dx * sin_r + dy * cos_r;
        }
    }

    let mut mesh = egui::Mesh::with_texture(texture_id);
    for i in 0..4 {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: positions[i],
            uv: uvs[i],
            color: egui::Color32::WHITE,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
}

/// 回転した画像を Mesh で描画する（90° 単位のみ）。
pub(crate) fn draw_rotated_image(
    painter: &egui::Painter,
    texture_id: egui::TextureId,
    rect: egui::Rect,
    rotation: crate::rotation_db::Rotation,
) {
    draw_rotated_image_ex(painter, texture_id, rect, rotation, 0.0, rect.center());
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
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    draw_thumb_texture(painter, inner, tex, rotation);
                    draw_folder_badge(painter, inner, name);
                }
                ThumbnailState::Pending | ThumbnailState::Evicted | ThumbnailState::Failed => {
                    painter.text(
                        inner.center() - egui::vec2(0.0, 14.0),
                        egui::Align2::CENTER_CENTER,
                        "📁",
                        egui::FontId::proportional(42.0),
                        egui::Color32::from_rgb(220, 170, 30),
                    );
                    painter.text(
                        egui::pos2(inner.center().x, inner.max.y - 4.0),
                        egui::Align2::CENTER_BOTTOM,
                        truncate_name(name, 18),
                        egui::FontId::proportional(11.0),
                        egui::Color32::from_gray(30),
                    );
                }
            }
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
        GridItem::ZipImage { .. } | GridItem::PdfPage { .. } => {
            draw_thumb(painter, inner, thumb, rotation);
        }
        GridItem::ZipFile(path) | GridItem::PdfFile(path) => {
            let (icon, badge_fn): (&str, fn(&egui::Painter, egui::Rect)) =
                if matches!(item, GridItem::ZipFile(_)) { ("📦", draw_zip_badge) } else { ("📄", draw_pdf_badge) };
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    draw_thumb_texture(painter, inner, tex, rotation);
                }
                ThumbnailState::Pending | ThumbnailState::Evicted | ThumbnailState::Failed => {
                    painter.rect_filled(inner, 2.0, egui::Color32::from_gray(230));
                    painter.text(
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        icon,
                        egui::FontId::proportional(32.0),
                        egui::Color32::from_gray(120),
                    );
                }
            }
            badge_fn(painter, inner);
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            painter.text(
                egui::pos2(inner.center().x, inner.max.y - 4.0),
                egui::Align2::CENTER_BOTTOM,
                truncate_name(name, 18),
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(30),
            );
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

/// egui::ColorImage → image::DynamicImage 変換ヘルパー。
/// AI 推論の入力に使う。
fn color_image_to_dynamic(ci: &egui::ColorImage) -> image::DynamicImage {
    let w = ci.size[0] as u32;
    let h = ci.size[1] as u32;
    let mut buf = image::RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let c = ci.pixels[(y * w + x) as usize];
            buf.put_pixel(x, y, image::Rgb([c.r(), c.g(), c.b()]));
        }
    }
    image::DynamicImage::ImageRgb8(buf)
}
