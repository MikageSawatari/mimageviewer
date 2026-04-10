use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};

use eframe::egui;

const SUPPORTED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif"];
const SUPPORTED_VIDEO_EXTENSIONS: &[&str] = &["mpg", "mpeg", "mp4", "avi", "mov", "mkv", "wmv"];

// -----------------------------------------------------------------------
// データモデル
// -----------------------------------------------------------------------

#[derive(Clone)]
pub enum GridItem {
    Folder(PathBuf),
    Image(PathBuf),
    Video(PathBuf),
}

impl GridItem {
    fn path(&self) -> &Path {
        match self {
            GridItem::Folder(p) | GridItem::Image(p) | GridItem::Video(p) => p,
        }
    }
    fn name(&self) -> &str {
        self.path().file_name().and_then(|n| n.to_str()).unwrap_or("")
    }
}

pub enum ThumbnailState {
    /// まだロードされていない
    Pending,
    /// 読み込み済みで GPU テクスチャとして保持中
    ///
    /// `from_cache = true` の場合は WebP キャッシュ (q=75) から復元した状態で、
    /// 段階 E のアイドル時アップグレードで元画像から再デコードされる対象になる。
    /// `rendered_at_px` は生成時の長辺ピクセル数で、現在のセルサイズと比較して
    /// 著しく小さい場合 (列数変更後など) もアップグレード対象になる。
    Loaded {
        tex: egui::TextureHandle,
        from_cache: bool,
        rendered_at_px: u32,
    },
    /// 読み込みに失敗した（再試行しない）
    #[allow(dead_code)]
    Failed,
    /// 段階 B: 先読み範囲外に出て GPU テクスチャを破棄済み
    /// 再び範囲内に入ったら再ロードされる
    Evicted,
}

/// フルスクリーン読み込みスレッドからUIスレッドへ送るメッセージ
enum FsLoadResult {
    /// 静止画（GIF・APNG の1フレーム目のみを含む）
    Static(egui::ColorImage),
    /// アニメーション: (フレーム画像, 表示時間[秒]) のベクタ
    Animated(Vec<(egui::ColorImage, f64)>),
}

/// フルスクリーンキャッシュエントリ
enum FsCacheEntry {
    Static(egui::TextureHandle),
    Animated {
        frames: Vec<(egui::TextureHandle, f64)>, // (texture, delay_secs)
        current_frame: usize,
        next_frame_at: f64, // ctx.input(|i| i.time) 基準
    },
}

// -----------------------------------------------------------------------
// App
// -----------------------------------------------------------------------

pub struct App {
    address: String,
    current_folder: Option<PathBuf>,
    items: Vec<GridItem>,
    thumbnails: Vec<ThumbnailState>,
    selected: Option<usize>,
    settings: crate::settings::Settings,
    tx: mpsc::Sender<ThumbMsg>,
    rx: mpsc::Receiver<ThumbMsg>,
    /// フォルダ移動時に true にセットすると旧ロードタスクが中断する
    cancel_token: Arc<AtomicBool>,
    /// Phase 2b ワーカーが参照する現在の可視先頭アイテムインデックス
    /// UIスレッドが毎フレーム更新し、バックグラウンドワーカーが優先度に使う
    scroll_hint: Arc<AtomicUsize>,

    /// スクロールオフセット（行境界にスナップ済み）。自前管理する
    scroll_offset_y: f32,
    /// 前フレームのセル幅（ = avail_w / cols）
    last_cell_size: f32,
    /// 前フレームのセル高さ（ = last_cell_size * thumb_aspect.height_ratio()）
    last_cell_h: f32,
    /// 前フレームのビューポート高さ（カーソルキースクロールに使用）
    last_viewport_h: f32,
    /// true のとき選択セルが見えるようにオフセットを調整する
    scroll_to_selected: bool,

    /// ウィンドウ状態保存用：最後に確認した outer_rect（最小化・最大化時は更新しない）
    last_outer_rect: Option<egui::Rect>,
    /// 現在のウィンドウの DPI スケール（論理→物理変換に使用）
    last_pixels_per_point: f32,

    /// キャッシュ生成進捗：新規デコードが必要だった画像の総数
    cache_gen_total: usize,
    /// キャッシュ生成進捗：完了した枚数（rayon スレッドからアトミックに更新）
    cache_gen_done: Arc<AtomicUsize>,

    // ── 段階 B: ページ単位先読み / eviction ──────────────────────
    /// アイテム idx → 画像メタデータ (mtime, file_size)。フォルダ・動画は None
    image_metas: Vec<Option<(i64, i64)>>,
    /// 永続ワーカーがサムネイルを処理するためのキュー（UI からは push のみ）
    reload_queue: Option<Arc<Mutex<Vec<LoadRequest>>>>,
    /// ロード要求を送ったがまだ応答が来ていない idx 集合（重複要求防止）。
    /// 値は `true` ならアイドル時アップグレード要求、`false` なら通常の読み込み要求。
    requested: std::collections::HashMap<usize, bool>,
    /// 現在の keep range [start, end)。update_keep_range で毎フレーム更新
    keep_range: (usize, usize),

    // ── 進捗バー (段階 B/E の合算進捗表示) ─────────────────────
    /// 現フレームで検出された通常読み込みのピーク件数 (current が 0 でリセット)
    progress_normal_peak: usize,
    /// 現フレームで検出された高画質化 (アイドルアップグレード) のピーク件数
    progress_upgrade_peak: usize,

    // ── 段階 E: アイドル時の画質向上 ─────────────────────────────
    /// 前フレームでの scroll_offset_y（変化検知用）
    last_scroll_offset_y_tracked: f32,
    /// 最後にスクロールが動いた瞬間の時刻（アイドル検出用）
    last_scroll_change_time: std::time::Instant,
    /// UI とワーカー間で共有する現在の display_px (列数変更時に追従させる)
    /// update_keep_range_and_requests で毎フレーム更新される。
    display_px_shared: Arc<AtomicU32>,

    // ── 統計情報 (起動時から累計) ─────────────────────────────
    /// サムネイル読み込みの統計 (時間分布・サイズ分布・フォーマット)。
    /// ワーカースレッドから Arc 経由で更新され、UI スレッドが読み出す。
    stats: Arc<Mutex<crate::stats::ThumbStats>>,
    /// 統計ダイアログの表示フラグ
    show_stats_dialog: bool,

    // ── フルスクリーン表示・先読みキャッシュ ───────────────────────
    /// Some(idx) = フルスクリーン表示中（self.items のインデックス）
    fullscreen_idx: Option<usize>,
    /// 先読みキャッシュ: item_idx → ロード済みエントリ（静止画 or アニメーション）
    fs_cache: std::collections::HashMap<usize, FsCacheEntry>,
    /// 先読み中: item_idx → (キャンセルトークン, 受信チャネル)
    fs_pending: std::collections::HashMap<usize, (Arc<AtomicBool>, mpsc::Receiver<FsLoadResult>)>,

    // ── お気に入り編集ポップアップ ────────────────────────────────
    show_favorites_editor: bool,

    // ── 環境設定ポップアップ ─────────────────────────────────────
    show_preferences: bool,
    /// 環境設定ダイアログ内の一時的な並列度編集値（Manual時の数値）
    pref_manual_threads: usize,

    // ── キャッシュ生成設定ポップアップ (段階 C) ──────────────────
    show_cache_policy_dialog: bool,

    // ── キャッシュ管理ポップアップ ───────────────────────────────
    show_cache_manager: bool,
    /// キャッシュ管理の「◯日以上古い」入力値
    cache_manager_days: u32,
    /// 開いたときに取得するキャッシュ統計: (フォルダ数, 合計バイト)
    cache_manager_stats: Option<(usize, u64)>,
    /// 削除後の結果メッセージ
    cache_manager_result: Option<String>,

    // ── 最後に選択した画像 (サムネイル画質ダイアログで使用) ──
    last_selected_image_path: Option<PathBuf>,

    // ── サムネイル画質設定ダイアログ ───────────────────────────
    show_thumb_quality_dialog: bool,
    /// サンプル画像 (デコード済み、ダイアログを閉じるまで保持)
    tq_sample: Option<image::DynamicImage>,
    /// サンプル画像のパス表示用
    tq_sample_path: Option<PathBuf>,
    /// サンプル画像の元ファイルサイズ (bytes)
    tq_sample_original_size: u64,
    /// パネル A: サイズ (long side px)
    tq_a_size: u32,
    /// パネル A: 品質 (1–100)
    tq_a_quality: u8,
    /// パネル A: プレビューテクスチャ
    tq_a_texture: Option<egui::TextureHandle>,
    /// パネル A: エンコード後のバイト数
    tq_a_bytes: usize,
    /// パネル B: サイズ
    tq_b_size: u32,
    /// パネル B: 品質
    tq_b_quality: u8,
    /// パネル B: プレビューテクスチャ
    tq_b_texture: Option<egui::TextureHandle>,
    /// パネル B: エンコード後のバイト数
    tq_b_bytes: usize,
    /// true = A/B 比較の全画面オーバーレイ表示中
    tq_fullscreen: bool,
    /// 全画面 A/B 比較時の縦線位置（0.0=すべて B、1.0=すべて A、中央は 0.5）
    tq_fs_divider: f32,

    // ── キャッシュ作成ポップアップ ───────────────────────────────
    show_cache_creator: bool,
    /// 各お気に入りのチェック状態（settings.favorites と同じ長さ）
    cache_creator_checked: Vec<bool>,
    /// 実行中フラグ（UI ボタンの有効/無効とポーリング制御）
    cache_creator_running: bool,
    /// カウントフェーズ中フラグ（total 未確定）
    cache_creator_counting: Arc<AtomicBool>,
    /// 対象フォルダ総数（Pass 1 完了後に確定）
    cache_creator_total: Arc<AtomicUsize>,
    /// 処理済みフォルダ数
    cache_creator_done: Arc<AtomicUsize>,
    /// キャッシュ容量 (バイト単位、累積加算)
    cache_creator_cache_size: Arc<AtomicU64>,
    /// キャンセルトークン
    cache_creator_cancel: Arc<AtomicBool>,
    /// 現在処理中のフォルダパス表示用
    cache_creator_current: Arc<Mutex<String>>,
    /// 完了シグナル（表示切替用）
    cache_creator_finished: Arc<AtomicBool>,
    /// 完了後のメッセージ
    cache_creator_result: Option<String>,

    // ── アドレスバーフォーカス管理 ───────────────────────────────
    /// true のときアドレスバーが入力中 → キーショートカットを無効化
    address_has_focus: bool,

    // ── フォルダ履歴（スクロール位置・選択状態の復元用）────────────
    /// フォルダパス → (scroll_offset_y, selected_idx)
    folder_history: std::collections::HashMap<PathBuf, (f32, Option<usize>)>,

    // ── 起動時の前回フォルダ復元フラグ ──────────────────────────
    initialized: bool,
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
            last_scroll_offset_y_tracked: 0.0,
            last_scroll_change_time: std::time::Instant::now(),
            display_px_shared: Arc::new(AtomicU32::new(512)),
            stats: Arc::new(Mutex::new(crate::stats::ThumbStats::new())),
            show_stats_dialog: false,
            fullscreen_idx: None,
            fs_cache: std::collections::HashMap::new(),
            fs_pending: std::collections::HashMap::new(),
            show_favorites_editor: false,
            show_preferences: false,
            pref_manual_threads: 4,
            show_cache_policy_dialog: false,
            show_cache_manager: false,
            cache_manager_days: 90,
            cache_manager_stats: None,
            cache_manager_result: None,
            last_selected_image_path: None,
            show_thumb_quality_dialog: false,
            tq_sample: None,
            tq_sample_path: None,
            tq_sample_original_size: 0,
            tq_a_size: 512,
            tq_a_quality: 75,
            tq_a_texture: None,
            tq_a_bytes: 0,
            tq_b_size: 512,
            tq_b_quality: 85,
            tq_b_texture: None,
            tq_b_bytes: 0,
            tq_fullscreen: false,
            tq_fs_divider: 0.5,
            show_cache_creator: false,
            cache_creator_checked: Vec::new(),
            cache_creator_running: false,
            cache_creator_counting: Arc::new(AtomicBool::new(false)),
            cache_creator_total: Arc::new(AtomicUsize::new(0)),
            cache_creator_done: Arc::new(AtomicUsize::new(0)),
            cache_creator_cache_size: Arc::new(AtomicU64::new(0)),
            cache_creator_cancel: Arc::new(AtomicBool::new(false)),
            cache_creator_current: Arc::new(Mutex::new(String::new())),
            cache_creator_finished: Arc::new(AtomicBool::new(false)),
            cache_creator_result: None,
            address_has_focus: false,
            folder_history: std::collections::HashMap::new(),
            initialized: false,
        }
    }
}

impl App {
    pub fn load_folder(&mut self, path: PathBuf) {
        crate::logger::log(format!("=== load_folder: {} ===", path.display()));

        // 現在のフォルダのスクロール位置・選択状態を履歴に保存
        if let Some(cur) = self.current_folder.clone() {
            self.folder_history.insert(cur, (self.scroll_offset_y, self.selected));
        }

        // フォルダ移動時はフルスクリーンを閉じる（先読みキャッシュも全クリア）
        self.close_fullscreen();

        // ── 旧タスクをキャンセル ──────────────────────────────────────
        self.cancel_token.store(true, Ordering::Relaxed);
        crate::logger::log("  cancel_token -> true (old tasks will stop)");
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_token = Arc::clone(&cancel);

        let (tx, rx) = mpsc::channel();
        self.tx = tx.clone();
        self.rx = rx;

        self.current_folder = Some(path.clone());
        self.address = path.to_string_lossy().to_string();
        self.selected = None;
        self.scroll_offset_y = 0.0;
        self.scroll_to_selected = false;
        self.scroll_hint.store(0, Ordering::Relaxed);

        // ── ディレクトリ走査（画像はメタデータも収集）────────────────
        let mut folders: Vec<GridItem> = Vec::new();
        // (path, is_video, mtime, file_size)
        let mut all_media: Vec<(PathBuf, bool, i64, i64)> = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    folders.push(GridItem::Folder(p));
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
                    }
                }
            }
        }

        folders.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
        match self.settings.sort_order {
            crate::settings::SortOrder::FileName => {
                all_media.sort_by(|(a, _, _, _), (b, _, _, _)| {
                    let a_name = a.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
                    let b_name = b.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
                    a_name.cmp(&b_name)
                });
            }
            crate::settings::SortOrder::Numeric => {
                all_media.sort_by(|(a, _, _, _), (b, _, _, _)| {
                    let a_name = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let b_name = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    natural_sort_key(a_name).cmp(&natural_sort_key(b_name))
                });
            }
            crate::settings::SortOrder::DateAsc => {
                all_media.sort_by(|(_, _, a_mtime, _), (_, _, b_mtime, _)| a_mtime.cmp(b_mtime));
            }
            crate::settings::SortOrder::DateDesc => {
                all_media.sort_by(|(_, _, a_mtime, _), (_, _, b_mtime, _)| b_mtime.cmp(a_mtime));
            }
        }

        // items: フォルダ先頭 → メディア（画像・動画を名前順混在）
        let folder_count = folders.len();
        self.items = folders;
        for (p, is_video, _, _) in &all_media {
            if *is_video {
                self.items.push(GridItem::Video(p.clone()));
            } else {
                self.items.push(GridItem::Image(p.clone()));
            }
        }
        self.thumbnails = (0..self.items.len()).map(|_| ThumbnailState::Pending).collect();
        self.requested.clear();
        self.keep_range = (0, 0);

        // 段階 B: アイテム idx と並行する画像メタデータ配列を構築
        // フォルダと動画は None、画像は Some((mtime, file_size))
        let mut image_metas: Vec<Option<(i64, i64)>> = Vec::with_capacity(self.items.len());
        for _ in 0..folder_count {
            image_metas.push(None);
        }
        for (_, is_video, mtime, file_size) in &all_media {
            if *is_video {
                image_metas.push(None);
            } else {
                image_metas.push(Some((*mtime, *file_size)));
            }
        }
        self.image_metas = image_metas;

        // 動画サムネイル用リスト: (item_idx, path)
        let video_items: Vec<(usize, PathBuf)> = all_media.iter()
            .enumerate()
            .filter_map(|(i, (p, is_video, _, _))| {
                if *is_video { Some((folder_count + i, p.clone())) } else { None }
            })
            .collect();

        // 画像リスト（カタログ掃除用）: (item_idx, path, mtime, file_size)
        let images: Vec<(usize, PathBuf, i64, i64)> = all_media.iter()
            .enumerate()
            .filter_map(|(i, (p, is_video, mtime, file_size))| {
                if !is_video { Some((folder_count + i, p.clone(), *mtime, *file_size)) } else { None }
            })
            .collect();

        // ── カタログを開いてキャッシュ状態を確認 ──────────────────────
        let cache_dir = crate::catalog::default_cache_dir();
        let catalog_arc: Option<std::sync::Arc<crate::catalog::CatalogDb>> =
            crate::catalog::CatalogDb::open(&cache_dir, &path)
                .map_err(|e| crate::logger::log(format!("  catalog open failed: {e}")))
                .ok()
                .map(std::sync::Arc::new);

        // 段階 B: 全キャッシュを Arc<HashMap> として一括ロードし、
        // 永続ワーカー群で共有する
        let cache_map: Arc<std::collections::HashMap<String, crate::catalog::CacheEntry>> =
            Arc::new(
                catalog_arc
                    .as_ref()
                    .and_then(|c| c.load_all().ok())
                    .unwrap_or_default(),
            );
        crate::logger::log(format!("  catalog: {} entries in DB", cache_map.len()));

        // 削除済みファイルのエントリを DB から掃除
        if let Some(ref cat) = catalog_arc {
            let existing: std::collections::HashSet<String> = images
                .iter()
                .filter_map(|(_, p, _, _)| p.file_name()?.to_str().map(String::from))
                .collect();
            if let Err(e) = cat.delete_missing(&existing) {
                crate::logger::log(format!("  catalog delete_missing failed: {e}"));
            }
        }

        // ── 進捗カウンタをリセット ────────────────────────────────────
        // 段階 B: 総数は動的になるので cache_gen_total は 0 にしておき、
        // タイトルバーには in-flight 件数を表示する
        self.cache_gen_total = 0;
        self.cache_gen_done = Arc::new(AtomicUsize::new(0));
        let cache_gen_done = Arc::clone(&self.cache_gen_done);

        // ── 永続ワーカーのセットアップ (段階 B) ───────────────────────
        // 共有要求キュー: UI スレッドが push、ワーカーが pop する
        let reload_queue: Arc<Mutex<Vec<LoadRequest>>> = Arc::new(Mutex::new(Vec::new()));
        self.reload_queue = Some(Arc::clone(&reload_queue));

        let thumb_threads = self.settings.parallelism.thread_count();
        crate::logger::log(format!("  spawning {thumb_threads} persistent workers"));

        // ワーカーに渡すパラメータ
        let scroll_hint = Arc::clone(&self.scroll_hint);
        let tx_for_video = tx.clone();
        let cancel_for_video = Arc::clone(&cancel);
        let thumb_px = self.settings.thumb_px;
        let thumb_quality = self.settings.thumb_quality;
        // 表示用 ColorImage の解像度を現在のセルサイズから算出 (段階 A)
        // Arc<AtomicU32> に格納してワーカー間で共有。列数変更時も追従させる。
        let initial_display_px = compute_display_px(
            self.last_cell_size,
            self.last_cell_h,
            self.last_pixels_per_point,
        );
        self.display_px_shared
            .store(initial_display_px, Ordering::Relaxed);
        // キャッシュ生成判定パラメータ (段階 C)
        let cache_decision = CacheDecision::from_settings(&self.settings);
        crate::logger::log(format!(
            "  display_px = {initial_display_px}  cache_policy = {}",
            self.settings.cache_policy.label()
        ));

        // 永続ワーカーを spawn（cancel されるまで reload_queue をポーリングし続ける）
        for worker_idx in 0..thumb_threads {
            let queue = Arc::clone(&reload_queue);
            let tx_w = tx.clone();
            let cancel_w = Arc::clone(&cancel);
            let hint_w = Arc::clone(&scroll_hint);
            let cache_map_w = Arc::clone(&cache_map);
            let catalog_w = catalog_arc.clone();
            let done_w = Arc::clone(&cache_gen_done);
            let display_px_w = Arc::clone(&self.display_px_shared);
            let stats_w = Arc::clone(&self.stats);

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
                            // ワーカーは各リクエスト処理直前に現在の display_px を読む
                            // → 列数変更で UI が値を更新したら即追従
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

        // ── 動画サムネイルを別スレッドで取得（Windows Shell API）─────────
        if !video_items.is_empty() {
            let tx_v = tx_for_video;
            let cancel_v = cancel_for_video;
            let thumb_size = self.last_cell_size.max(256.0) as i32;
            let stats_v = Arc::clone(&self.stats);
            // 動画用にメタデータ (file_size) を事前に収集しておく
            let video_sizes: std::collections::HashMap<usize, u64> = all_media
                .iter()
                .enumerate()
                .filter_map(|(i, (_, is_vid, _, size))| {
                    if *is_vid { Some((folder_count + i, (*size).max(0) as u64)) } else { None }
                })
                .collect();
            std::thread::spawn(move || {
                for (idx, path) in video_items {
                    if cancel_v.load(Ordering::Relaxed) { break; }
                    let ci = crate::video_thumb::get_video_thumbnail(&path, thumb_size);
                    crate::logger::log(format!(
                        "  video thumb: idx={idx} {}",
                        if ci.is_some() { "ok" } else { "FAIL" }
                    ));
                    // 統計: 動画件数 + サイズを記録 (成功のみ)
                    if ci.is_some() {
                        if let Ok(mut s) = stats_v.lock() {
                            let size = video_sizes.get(&idx).copied().unwrap_or(0);
                            s.record_video(size);
                        }
                    } else if let Ok(mut s) = stats_v.lock() {
                        s.record_failed();
                    }
                    // 動画 Shell API はアップグレード経路を持たないので from_cache = false
                    let _ = tx_v.send((idx, ci, false));
                }
            });
        }

        // 履歴があればスクロール位置・選択状態を復元
        if let Some(&(scroll, sel)) = self.folder_history.get(&path) {
            self.scroll_offset_y = scroll;
            self.selected = sel;
            if sel.is_some() {
                self.scroll_to_selected = true;
            }
        }

        // 前回フォルダとして保存
        self.settings.last_folder = Some(path);
        self.settings.save();
    }

    fn poll_thumbnails(&mut self, ctx: &egui::Context) {
        let mut count = 0u32;
        let (keep_start, keep_end) = self.keep_range;
        while let Ok((i, color_image_opt, from_cache)) = self.rx.try_recv() {
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
                            self.thumbnails[i] = ThumbnailState::Loaded {
                                tex: handle,
                                from_cache,
                                rendered_at_px,
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
        let cur_first = (self.scroll_offset_y / cell_h) as usize * cols;

        let prev_pages = self.settings.thumb_prev_pages as usize;
        let next_pages = self.settings.thumb_next_pages as usize;

        let mut keep_start = cur_first.saturating_sub(prev_pages * items_per_page);
        let mut keep_end = cur_first
            .saturating_add((1 + next_pages) * items_per_page)
            .min(total);

        // ── 段階 D: VRAM 安全ネット ──────────────────────────────────
        // display_px から 1 枚あたりの推定バイト数を算出し、cap を超えそうなら
        // keep_range を cur_first 中心に縮小する (前方 2/3 優先、後方 1/3)
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
                    let new_start = cur_first.saturating_sub(half_back);
                    let new_end = cur_first.saturating_add(half_forward).min(total);
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
            // 画像のみ要求。フォルダは描画時にアイコン、動画は別パス
            let Some((mtime, file_size)) = self.image_metas.get(i).copied().flatten() else {
                continue;
            };
            let path = match self.items.get(i) {
                Some(GridItem::Image(p)) => p.clone(),
                _ => continue,
            };
            new_requests.push(LoadRequest {
                idx: i, path, mtime, file_size,
                skip_cache: false,
            });
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
        const BATCH: usize = 4;
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

        // 候補集め: keep_range 内で from_cache=true or 解像度不足のものを最大 BATCH 件
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
            let path = match self.items.get(i) {
                Some(GridItem::Image(p)) => p.clone(),
                _ => continue,
            };
            upgrade_reqs.push(LoadRequest {
                idx: i, path, mtime, file_size,
                skip_cache: true,
            });
            if upgrade_reqs.len() >= BATCH {
                break;
            }
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

    /// 現在の要求状況からプログレスバーのピーク値を更新する。
    ///
    /// ピークは "current == 0 のときに 0 にリセット、それ以外は current の最大値" で
    /// 計算される。UI は `(peak - current) / peak` で進捗率を表示する。
    fn update_progress_peaks(&mut self) {
        // in-flight を種類別にカウント
        let (in_flight_normal, in_flight_upgrade) = {
            let mut n = 0usize;
            let mut u = 0usize;
            for &is_upgrade in self.requested.values() {
                if is_upgrade { u += 1; } else { n += 1; }
            }
            (n, u)
        };

        // キュー内を種類別にカウント (短時間ロック)
        let (queued_normal, queued_upgrade) = if let Some(queue) = &self.reload_queue {
            let q = queue.lock().unwrap();
            let upgrade = q.iter().filter(|r| r.skip_cache).count();
            let normal = q.len() - upgrade;
            (normal, upgrade)
        } else {
            (0, 0)
        };

        let cur_normal = in_flight_normal + queued_normal;
        let cur_upgrade = in_flight_upgrade + queued_upgrade;

        // ピーク更新 or リセット
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
    fn progress_snapshot(&self) -> ((usize, usize), (usize, usize)) {
        // peak は update_progress_peaks 時点の値
        // current は再計算 (毎フレーム少しズレるが UX への影響は無視できる)
        let (in_flight_normal, in_flight_upgrade) = {
            let mut n = 0usize;
            let mut u = 0usize;
            for &is_upgrade in self.requested.values() {
                if is_upgrade { u += 1; } else { n += 1; }
            }
            (n, u)
        };
        let (queued_normal, queued_upgrade) = if let Some(queue) = &self.reload_queue {
            let q = queue.lock().unwrap();
            let upgrade = q.iter().filter(|r| r.skip_cache).count();
            let normal = q.len() - upgrade;
            (normal, upgrade)
        } else {
            (0, 0)
        };
        (
            (in_flight_normal + queued_normal, self.progress_normal_peak),
            (in_flight_upgrade + queued_upgrade, self.progress_upgrade_peak),
        )
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        // フルスクリーン表示中はサムネイルグリッドのキー操作を無効化
        // （フルスクリーンビューポートが独自に処理する）
        if self.fullscreen_idx.is_some() {
            return None;
        }
        // アドレスバー入力中はすべてのショートカットを無効化
        if self.address_has_focus {
            return None;
        }

        let cols = self.settings.grid_cols.max(1);
        let n = self.items.len();

        let (right, left, down, up, enter, backspace, ctrl_up, ctrl_down) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Backspace),
                i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowUp),
                i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowDown),
            )
        });

        if n > 0 {
            let sel = self.selected.unwrap_or(0);
            let new_sel = if right {
                Some((sel + 1).min(n - 1))
            } else if left {
                Some(sel.saturating_sub(1))
            } else if down {
                Some((sel + cols).min(n - 1))
            } else if up {
                Some(sel.saturating_sub(cols))
            } else {
                None
            };

            if let Some(s) = new_sel {
                self.selected = Some(s);
                self.scroll_to_selected = true;
                self.update_last_selected_image();
            }

            if enter {
                if let Some(idx) = self.selected {
                    match self.items.get(idx) {
                        Some(GridItem::Folder(p)) => return Some(p.clone()),
                        Some(GridItem::Image(_)) => self.open_fullscreen(idx),
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
    fn apply_scroll_to_selected(&mut self, cols: usize, cell_h: f32) {
        let sel = match self.selected {
            Some(s) => s,
            None => return,
        };
        let row = sel / cols;
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
            Some(GridItem::Image(_)) => {
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
            _ => {}
        }
    }

    /// 1枚のフルサイズ画像を非同期で読み込み開始する。
    /// GIF / APNG はアニメーションフレームを全デコードして FsLoadResult::Animated を送信する。
    fn start_fs_load(&mut self, idx: usize) {
        if let Some(GridItem::Image(path)) = self.items.get(idx) {
            let path = path.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            let (tx, rx) = mpsc::channel::<FsLoadResult>();
            self.fs_pending.insert(idx, (Arc::clone(&cancel), rx));

            std::thread::spawn(move || {
                if cancel.load(Ordering::Relaxed) { return; }
                let t = std::time::Instant::now();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

                // GIF: アニメーション試行
                if ext == "gif" {
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

                // PNG: APNG アニメーション試行
                if ext == "png" {
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
                match image::open(&path) {
                    Ok(img) => {
                        // TODO Phase 2: NGX DLISR アップスケール統合ポイント
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
                    }
                }
            });
        }
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

    /// items の中の画像アイテムの item_idx 一覧を返す（先読みウィンドウ用）
    fn collect_image_indices(items: &[GridItem]) -> Vec<usize> {
        items.iter().enumerate()
            .filter_map(|(i, item)| matches!(item, GridItem::Image(_)).then_some(i))
            .collect()
    }


    /// フルスクリーン表示を終了し、先読みキャッシュを全クリアする。
    fn close_fullscreen(&mut self) {
        self.fullscreen_idx = None;
        for (cancel, _) in self.fs_pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.fs_pending.clear();
        self.fs_cache.clear();
    }

    /// `self.selected` に対応するアイテムが画像の場合、パスを last_selected_image_path に保存する。
    /// (フォルダ移動後もサムネイル画質ダイアログで使えるよう、セッション内で保持)
    fn update_last_selected_image(&mut self) {
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
    fn open_thumb_quality_dialog(&mut self, ctx: &egui::Context) {
        // 既存状態をリセット
        self.tq_sample = None;
        self.tq_sample_path = None;
        self.tq_sample_original_size = 0;
        self.tq_a_texture = None;
        self.tq_b_texture = None;
        self.tq_a_bytes = 0;
        self.tq_b_bytes = 0;

        // 最後に選択した画像を取得
        let Some(path) = self.last_selected_image_path.clone() else {
            // None のままダイアログを開く (メッセージだけ出る)
            self.show_thumb_quality_dialog = true;
            return;
        };

        // サンプル画像をデコード
        let img = match image::open(&path) {
            Ok(i) => i,
            Err(_) => {
                self.show_thumb_quality_dialog = true;
                return;
            }
        };
        let orig_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        self.tq_sample = Some(img);
        self.tq_sample_path = Some(path);
        self.tq_sample_original_size = orig_size;

        // 現在の設定で A を初期化、B はちょっと違う組み合わせ
        self.tq_a_size = self.settings.thumb_px;
        self.tq_a_quality = self.settings.thumb_quality;
        self.tq_b_size = self.settings.thumb_px;
        self.tq_b_quality = (self.settings.thumb_quality as u32 + 10).min(95) as u8;

        self.reencode_tq_panel(ctx, true);
        self.reencode_tq_panel(ctx, false);
        self.show_thumb_quality_dialog = true;
    }

    fn reencode_tq_panel(&mut self, ctx: &egui::Context, is_a: bool) {
        let Some(img) = self.tq_sample.as_ref() else { return };
        let (size, quality) = if is_a {
            (self.tq_a_size, self.tq_a_quality)
        } else {
            (self.tq_b_size, self.tq_b_quality)
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
            self.tq_a_bytes = bytes;
            self.tq_a_texture = tex;
        } else {
            self.tq_b_bytes = bytes;
            self.tq_b_texture = tex;
        }
    }

    fn close_thumb_quality_dialog(&mut self) {
        self.show_thumb_quality_dialog = false;
        self.tq_sample = None;
        self.tq_sample_path = None;
        self.tq_a_texture = None;
        self.tq_b_texture = None;
        self.tq_fullscreen = false;
    }

    // -------------------------------------------------------------------
    // キャッシュ作成（バックグラウンドで選択フォルダ以下を再帰処理）
    // -------------------------------------------------------------------
    fn start_cache_creation(&mut self) {
        // 選択されたお気に入りを集める
        let targets: Vec<PathBuf> = self
            .settings
            .favorites
            .iter()
            .zip(self.cache_creator_checked.iter())
            .filter_map(|(p, &c)| if c { Some(p.clone()) } else { None })
            .collect();

        if targets.is_empty() {
            return;
        }

        // 状態リセット
        self.cache_creator_running = true;
        self.cache_creator_counting.store(true, Ordering::Relaxed);
        self.cache_creator_total.store(0, Ordering::Relaxed);
        self.cache_creator_done.store(0, Ordering::Relaxed);
        self.cache_creator_finished.store(false, Ordering::Relaxed);
        self.cache_creator_result = None;
        *self.cache_creator_current.lock().unwrap() = String::new();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cache_creator_cancel = Arc::clone(&cancel);

        // 初期キャッシュ容量を取得（ベースライン）
        let cache_dir = crate::catalog::default_cache_dir();
        let (_, baseline) = crate::catalog::cache_stats(&cache_dir);
        self.cache_creator_cache_size
            .store(baseline, Ordering::Relaxed);

        // atomic クローン
        let counting = Arc::clone(&self.cache_creator_counting);
        let total = Arc::clone(&self.cache_creator_total);
        let done = Arc::clone(&self.cache_creator_done);
        let size_atomic = Arc::clone(&self.cache_creator_cache_size);
        let finished = Arc::clone(&self.cache_creator_finished);
        let current = Arc::clone(&self.cache_creator_current);
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
                        if !p.is_file() {
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
        if !self.initialized {
            self.initialized = true;
            if let Some(folder) = self.settings.last_folder.clone() {
                if folder.is_dir() {
                    self.load_folder(folder);
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

        self.poll_thumbnails(ctx);
        self.update_keep_range_and_requests();
        self.poll_prefetch(ctx);

        // タイトルバーは常にアプリ名のみ。進捗は UI 内のプログレスバーで表示する。
        ctx.send_viewport_cmd(egui::ViewportCommand::Title("mimageviewer".to_string()));

        // スクロールは egui に触れる前に処理（イベントを消費）
        self.process_scroll(ctx);

        let keyboard_nav = self.handle_keyboard(ctx);

        // ── フルスクリーンビューポート ──────────────────────────────────
        if let Some(fs_idx) = self.fullscreen_idx {
            // 動画か否かを判定
            let is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
            let video_path = if is_video {
                if let Some(GridItem::Video(p)) = self.items.get(fs_idx) {
                    Some(p.clone())
                } else { None }
            } else { None };

            // アニメーションフレームを進める（メインコンテキストの時刻を使う）
            if !is_video {
                let now = ctx.input(|i| i.time);
                if let Some(FsCacheEntry::Animated { frames, current_frame, next_frame_at }) =
                    self.fs_cache.get_mut(&fs_idx)
                {
                    if now >= *next_frame_at && !frames.is_empty() {
                        *current_frame = (*current_frame + 1) % frames.len();
                        let delay = frames[*current_frame].1.max(0.02);
                        *next_frame_at = now + delay;
                    }
                }
            }

            // 表示テクスチャを取得（動画は None、画像はキャッシュエントリから）
            let tex: Option<egui::TextureHandle> = if is_video {
                None
            } else {
                match self.fs_cache.get(&fs_idx) {
                    Some(FsCacheEntry::Static(h)) => Some(h.clone()),
                    Some(FsCacheEntry::Animated { frames, current_frame, .. }) => {
                        frames.get(*current_frame).map(|(h, _)| h.clone())
                    }
                    None => None,
                }
            };

            let thumb_tex  = match self.thumbnails.get(fs_idx) {
                Some(ThumbnailState::Loaded { tex, .. }) => Some(tex.clone()),
                _ => None,
            };
            let filename   = self.items.get(fs_idx)
                .map(|item| item.name().to_string())
                .unwrap_or_default();
            // 画像のみ「高解像度読込中」表示が必要（動画は不要）
            let is_loading = !is_video && !self.fs_cache.contains_key(&fs_idx);

            let mut close_fs   = false;
            let mut nav_delta: i32     = 0;
            let mut ctrl_nav: Option<i32> = None;

            // メインウィンドウがあるモニターの論理ピクセル矩形を取得し、
            // そのモニターを完全に覆う borderless ウィンドウを作成する。
            // with_fullscreen(true) はプライマリモニター固定になるため使わない。
            let fs_builder = {
                let center = self.last_outer_rect.map(|r| r.center());
                let ppp = self.last_pixels_per_point;
                crate::logger::log(format!(
                    "[fullscreen] last_outer_rect center: {:?}  ppp={ppp:.2}",
                    center.map(|c| format!("({:.1},{:.1})", c.x, c.y))
                ));

                // MonitorFromPoint は物理座標を要求するため論理座標に ppp を乗算する
                let monitor_rect = center
                    .and_then(|c| crate::monitor::get_monitor_logical_rect_at(
                        c.x * ppp, c.y * ppp,
                    ));

                let b = egui::ViewportBuilder::default().with_decorations(false);
                match monitor_rect {
                    Some(rect) => {
                        crate::logger::log(format!(
                            "[fullscreen] using monitor rect: pos=({:.1},{:.1}) size={:.1}x{:.1}",
                            rect.min.x, rect.min.y, rect.width(), rect.height()
                        ));
                        b.with_position(rect.min)
                         .with_inner_size([rect.width(), rect.height()])
                    }
                    None => {
                        crate::logger::log("[fullscreen] monitor rect not found, fallback to with_fullscreen".to_string());
                        b.with_fullscreen(true)
                    }
                }
            };

            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("fullscreen_viewer"),
                fs_builder,
                |ctx, _class| {
                    // プラットフォームの閉じるリクエスト（Alt+F4 など）
                    if ctx.input(|i| i.viewport().close_requested()) {
                        close_fs = true;
                    }

                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                        .show(ctx, |ui| {
                            let full_rect = ui.max_rect();

                            // ── キー入力（ctx はこのビューポートのコンテキスト）
                            let esc    = ctx.input(|i| i.key_pressed(egui::Key::Escape));
                            let right  = ctx.input(|i| {
                                i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowDown)
                            });
                            let left   = ctx.input(|i| {
                                i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowUp)
                            });
                            let ctrl_d = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowDown));
                            let ctrl_u = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowUp));

                            if esc              { close_fs = true; }
                            if right && !ctrl_d { nav_delta =  1; }
                            if left  && !ctrl_u { nav_delta = -1; }
                            if ctrl_d           { ctrl_nav = Some(1); }
                            if ctrl_u           { ctrl_nav = Some(-1); }

                            // ── ホイール操作 ──────────────────────────
                            // 下スクロール(delta<0) → 次の画像、上スクロール(delta>0) → 前の画像
                            let wheel_y = ctx.input(|i| i.raw_scroll_delta.y);
                            if wheel_y.abs() > 0.5 {
                                ctx.input_mut(|i| {
                                    i.raw_scroll_delta = egui::Vec2::ZERO;
                                    i.smooth_scroll_delta = egui::Vec2::ZERO;
                                    i.events.retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
                                });
                                nav_delta = if wheel_y < 0.0 { 1 } else { -1 };
                            }

                            // ── マウスクリック操作 ────────────────────
                            let fs_response = ui.interact(
                                full_rect,
                                egui::Id::new("fs_click"),
                                egui::Sense::click(),
                            );
                            if is_video {
                                // 動画: クリックで外部プレイヤー起動
                                if fs_response.clicked() {
                                    if let Some(ref vp) = video_path {
                                        open_external_player(vp);
                                    }
                                }
                            } else {
                                // 画像: 左半分 → 前、右半分 → 次
                                if fs_response.clicked() {
                                    if let Some(pos) = fs_response.interact_pointer_pos() {
                                        if pos.x > full_rect.center().x {
                                            nav_delta = 1;
                                        } else {
                                            nav_delta = -1;
                                        }
                                    }
                                }
                            }
                            // 右クリックでフルスクリーン終了 → サムネイル一覧に戻る
                            if fs_response.secondary_clicked() {
                                close_fs = true;
                            }

                            // ── 画像 / 動画表示 ───────────────────────
                            // 動画はサムネイルのみ表示。画像はフルサイズ優先。
                            let display_tex = tex.as_ref().or(thumb_tex.as_ref());
                            if let Some(handle) = display_tex {
                                let tex_size = handle.size_vec2();
                                let scale    = (full_rect.width()  / tex_size.x)
                                               .min(full_rect.height() / tex_size.y);
                                let img_rect = egui::Rect::from_center_size(
                                    full_rect.center(),
                                    tex_size * scale,
                                );
                                ui.painter().image(
                                    handle.id(),
                                    img_rect,
                                    egui::Rect::from_min_max(
                                        egui::pos2(0.0, 0.0),
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                            } else {
                                // テクスチャ未ロード（サムネイルも未完了）
                                ui.painter().text(
                                    full_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    if is_video { "動画サムネイル 読込中..." } else { "読込中..." },
                                    egui::FontId::proportional(24.0),
                                    egui::Color32::from_gray(180),
                                );
                            }

                            // ── 動画: 再生ボタンオーバーレイ ─────────
                            if is_video {
                                draw_play_icon(ui.painter(), full_rect.center(), 56.0);
                                // Enter キーでも起動
                                let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
                                if enter {
                                    if let Some(ref vp) = video_path {
                                        open_external_player(vp);
                                    }
                                }
                            }

                            // サムネイル仮表示中 → 高解像度読み込み中インジケーター（画像のみ）
                            if is_loading && display_tex.is_some() {
                                ui.painter().text(
                                    full_rect.min + egui::vec2(16.0, 16.0),
                                    egui::Align2::LEFT_TOP,
                                    "高解像度 読込中...",
                                    egui::FontId::proportional(14.0),
                                    egui::Color32::from_rgba_unmultiplied(220, 220, 220, 180),
                                );
                            }

                            // ファイル名を右下に表示
                            if !filename.is_empty() {
                                ui.painter().text(
                                    full_rect.max - egui::vec2(16.0, 16.0),
                                    egui::Align2::RIGHT_BOTTOM,
                                    &filename,
                                    egui::FontId::proportional(14.0),
                                    egui::Color32::from_rgba_unmultiplied(220, 220, 220, 200),
                                );
                            }

                            // ── ホバー時の閉じるボタン（右上）────────
                            // 画面上部 60px にマウスがあるときだけ表示する
                            let hover_in_top = ctx.input(|i| {
                                i.pointer.hover_pos()
                                    .map(|p| p.y < 60.0)
                                    .unwrap_or(false)
                            });
                            if hover_in_top {
                                let btn_size = 40.0;
                                let margin = 10.0;
                                let btn_rect = egui::Rect::from_min_size(
                                    egui::pos2(
                                        full_rect.max.x - btn_size - margin,
                                        margin,
                                    ),
                                    egui::vec2(btn_size, btn_size),
                                );
                                let btn_resp = ui.interact(
                                    btn_rect,
                                    egui::Id::new("fs_close_btn"),
                                    egui::Sense::click(),
                                );
                                let bg = if btn_resp.hovered() {
                                    egui::Color32::from_rgba_unmultiplied(220, 50, 50, 230)
                                } else {
                                    egui::Color32::from_rgba_unmultiplied(40, 40, 40, 180)
                                };
                                ui.painter().rect_filled(btn_rect, 6.0, bg);
                                // ✕ をフォントに依存せず斜め2線で描画
                                let c = btn_rect.center();
                                let r = btn_size * 0.25;
                                let stroke = egui::Stroke::new(2.5, egui::Color32::WHITE);
                                ui.painter().line_segment(
                                    [egui::pos2(c.x - r, c.y - r), egui::pos2(c.x + r, c.y + r)],
                                    stroke,
                                );
                                ui.painter().line_segment(
                                    [egui::pos2(c.x + r, c.y - r), egui::pos2(c.x - r, c.y + r)],
                                    stroke,
                                );
                                if btn_resp.clicked() {
                                    close_fs = true;
                                }
                                // ×ボタンのクリックが背面の nav_delta に漏れないように上書き
                                if btn_resp.hovered() {
                                    nav_delta = 0;
                                }
                            }
                        });
                },
            );

            // ── フルスクリーン終了・ナビゲーション処理 ────────────────
            if close_fs || ctrl_nav.is_some() {
                self.close_fullscreen();
            }
            if let Some(delta) = ctrl_nav {
                // Ctrl+↑↓: フォルダを移動してサムネイルモードに戻る（仕様 §7.2）
                if let Some(cur) = self.current_folder.clone() {
                    let skip_limit = self.settings.folder_skip_limit;
                    let next = if delta > 0 {
                        navigate_folder_with_skip(&cur, next_folder_dfs, skip_limit)
                    } else {
                        navigate_folder_with_skip(&cur, prev_folder_dfs, skip_limit)
                    };
                    if let Some(p) = next {
                        self.load_folder(p);
                    }
                }
            } else if !close_fs && nav_delta != 0 {
                // ←→↑↓: 画像・動画を前後に切り替え
                if let Some(new_idx) = adjacent_navigable_idx(&self.items, fs_idx, nav_delta) {
                    self.open_fullscreen(new_idx);
                    self.selected = Some(new_idx);
                    self.scroll_to_selected = true;
                    self.update_last_selected_image();
                }
            }

            // 高解像度読み込み完了まで毎フレーム再描画（画像のみ）
            let image_loading = !is_video
                && self.fullscreen_idx.map(|i| !self.fs_cache.contains_key(&i)).unwrap_or(false);
            if image_loading {
                ctx.request_repaint();
            }

            // アニメーション: 次フレームの時刻まで待ってから再描画
            if !is_video {
                if let Some(FsCacheEntry::Animated { next_frame_at, .. }) =
                    self.fs_cache.get(&fs_idx)
                {
                    let delay = (next_frame_at - ctx.input(|i| i.time)).max(0.0);
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay));
                }
            }
        }

        // ── メニューバー ─────────────────────────────────────────────
        let mut fav_nav: Option<PathBuf> = None;
        let mut settings_changed = false;
        let mut sort_changed = false;
        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("ファイル", |ui| {
                    if ui.button("終了").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button("お気に入り", |ui| {
                    // このフォルダを追加
                    let can_add = self.current_folder.is_some();
                    if ui.add_enabled(can_add, egui::Button::new("このフォルダを追加")).clicked() {
                        if let Some(ref folder) = self.current_folder.clone() {
                            if self.settings.add_favorite(folder.clone()) {
                                self.settings.save();
                            }
                        }
                        ui.close();
                    }

                    // 編集
                    if ui.button("編集").clicked() {
                        self.show_favorites_editor = true;
                        ui.close();
                    }

                    // キャッシュ作成
                    if ui.button("キャッシュ作成").clicked() {
                        self.cache_creator_checked = vec![false; self.settings.favorites.len()];
                        self.cache_creator_running = false;
                        self.cache_creator_result = None;
                        self.cache_creator_total.store(0, Ordering::Relaxed);
                        self.cache_creator_done.store(0, Ordering::Relaxed);
                        self.cache_creator_cache_size.store(0, Ordering::Relaxed);
                        self.cache_creator_finished.store(false, Ordering::Relaxed);
                        *self.cache_creator_current.lock().unwrap() = String::new();
                        self.show_cache_creator = true;
                        ui.close();
                    }

                    // 区切り線
                    ui.separator();

                    // 登録済みお気に入り一覧
                    if self.settings.favorites.is_empty() {
                        ui.label(egui::RichText::new("（未登録）").weak());
                    } else {
                        let favorites = self.settings.favorites.clone();
                        for fav in &favorites {
                            let label = fav.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or_else(|| fav.to_str().unwrap_or("?"));
                            if ui.button(label).clicked() {
                                fav_nav = Some(fav.clone());
                                ui.close();
                            }
                        }
                    }
                });

                ui.menu_button("設定", |ui| {
                    ui.menu_button("サムネイル列数", |ui| {
                        for cols in 2..=10usize {
                            let checked = self.settings.grid_cols == cols;
                            let prefix = if checked { "✓ " } else { "  " };
                            if ui.button(format!("{prefix}{cols} 列")).clicked() {
                                self.settings.grid_cols = cols;
                                settings_changed = true;
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("サムネイル比率", |ui| {
                        for &aspect in crate::settings::ThumbAspect::all() {
                            let checked = self.settings.thumb_aspect == aspect;
                            let prefix = if checked { "✓ " } else { "  " };
                            if ui.button(format!("{prefix}{}", aspect.label())).clicked() {
                                self.settings.thumb_aspect = aspect;
                                settings_changed = true;
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("ソート順", |ui| {
                        for &order in crate::settings::SortOrder::all() {
                            let checked = self.settings.sort_order == order;
                            let prefix = if checked { "✓ " } else { "  " };
                            if ui.button(format!("{prefix}{}", order.label())).clicked() {
                                self.settings.sort_order = order;
                                sort_changed = true;
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("キャッシュ管理").clicked() {
                        let cache_dir = crate::catalog::default_cache_dir();
                        self.cache_manager_stats = Some(crate::catalog::cache_stats(&cache_dir));
                        self.cache_manager_result = None;
                        self.show_cache_manager = true;
                        ui.close();
                    }
                    if ui.button("サムネイル画質…").clicked() {
                        self.open_thumb_quality_dialog(ctx);
                        ui.close();
                    }
                    if ui.button("キャッシュ生成設定…").clicked() {
                        self.show_cache_policy_dialog = true;
                        ui.close();
                    }
                    if ui.button("統計…").clicked() {
                        self.show_stats_dialog = true;
                        ui.close();
                    }
                    if ui.button("環境設定…").clicked() {
                        // ダイアログを開くとき現在値で初期化
                        self.pref_manual_threads = match &self.settings.parallelism {
                            crate::settings::Parallelism::Manual(n) => *n,
                            crate::settings::Parallelism::Auto => {
                                self.settings.parallelism.thread_count()
                            }
                        };
                        self.show_preferences = true;
                        ui.close();
                    }
                });
            });
        });
        if settings_changed {
            self.settings.save();
        }
        if sort_changed {
            self.settings.save();
            if let Some(path) = self.current_folder.clone() {
                // スクロール履歴を捨てて先頭から再ロード
                self.folder_history.remove(&path);
                self.load_folder(path);
            }
        }

        // ── 進捗バー (メニュー直下、処理中のみ表示) ──────────────────
        let ((cur_normal, peak_normal), (cur_upgrade, peak_upgrade)) =
            self.progress_snapshot();
        if peak_normal > 0 || peak_upgrade > 0 {
            egui::TopBottomPanel::top("progress_panel").show(ctx, |ui| {
                ui.add_space(3.0);
                if peak_normal > 0 {
                    let done = peak_normal.saturating_sub(cur_normal);
                    let progress = done as f32 / peak_normal as f32;
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("先読み    ").monospace());
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .desired_width(280.0)
                                .fill(egui::Color32::from_rgb(60, 130, 220))
                                .text(
                                    egui::RichText::new(format!(
                                        "{} / {}",
                                        done, peak_normal
                                    ))
                                    .color(egui::Color32::WHITE),
                                ),
                        );
                    });
                }
                if peak_upgrade > 0 {
                    let done = peak_upgrade.saturating_sub(cur_upgrade);
                    let progress = done as f32 / peak_upgrade as f32;
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("高画質化  ").monospace());
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .desired_width(280.0)
                                .fill(egui::Color32::from_rgb(100, 170, 240))
                                .text(
                                    egui::RichText::new(format!(
                                        "{} / {}",
                                        done, peak_upgrade
                                    ))
                                    .color(egui::Color32::WHITE),
                                ),
                        );
                    });
                }
                ui.add_space(3.0);
            });
            // 進行中は毎フレーム再描画してバーをスムーズに更新
            ctx.request_repaint();
        }

        // ── お気に入り編集ポップアップ ───────────────────────────────
        if self.show_favorites_editor {
            let mut open = true;
            let mut swap: Option<(usize, usize)> = None;
            let mut remove: Option<usize> = None;
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            egui::Window::new("お気に入りの編集")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(360.0);
                    if self.settings.favorites.is_empty() {
                        ui.label("お気に入りはまだ登録されていません。");
                    } else {
                        let n = self.settings.favorites.len();
                        egui::Grid::new("fav_edit_grid")
                            .striped(true)
                            .num_columns(2)
                            .show(ui, |ui| {
                                for i in 0..n {
                                    let path_str = self.settings.favorites[i].to_string_lossy().to_string();
                                    ui.label(&path_str);
                                    ui.horizontal(|ui| {
                                        let up_en = i > 0;
                                        let dn_en = i + 1 < n;
                                        if ui.add_enabled(up_en, egui::Button::new("↑")).clicked() {
                                            swap = Some((i - 1, i));
                                        }
                                        if ui.add_enabled(dn_en, egui::Button::new("↓")).clicked() {
                                            swap = Some((i, i + 1));
                                        }
                                        if ui.button("削除").clicked() {
                                            remove = Some(i);
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                    }
                });

            if let Some((a, b)) = swap {
                self.settings.favorites.swap(a, b);
                self.settings.save();
            }
            if let Some(i) = remove {
                self.settings.favorites.remove(i);
                self.settings.save();
            }
            if !open {
                self.show_favorites_editor = false;
            }
        }

        // ── キャッシュ管理ポップアップ ───────────────────────────────
        if self.show_cache_manager {
            let mut open = true;
            let cache_dir = crate::catalog::default_cache_dir();
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            egui::Window::new("キャッシュ管理")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(380.0);

                    // ── 統計表示 ──────────────────────────────────
                    if let Some((folders, bytes)) = self.cache_manager_stats {
                        let size_str = if bytes >= 1024 * 1024 * 1024 {
                            format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
                        } else {
                            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
                        };
                        ui.label(format!("キャッシュ: {folders} フォルダ / {size_str}"));
                    } else {
                        ui.label("キャッシュ情報を取得中...");
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // ── 古いキャッシュの削除 ──────────────────────
                    ui.horizontal(|ui| {
                        let mut days_str = self.cache_manager_days.to_string();
                        ui.label("最終更新から");
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut days_str)
                                .desired_width(48.0)
                                .horizontal_align(egui::Align::Center),
                        );
                        if resp.changed() {
                            if let Ok(v) = days_str.parse::<u32>() {
                                if v > 0 {
                                    self.cache_manager_days = v;
                                }
                            }
                        }
                        ui.label("日以上更新がないキャッシュを削除する");
                    });
                    ui.add_space(4.0);
                    if ui.button(format!("  {} 日以上古いキャッシュを削除  ", self.cache_manager_days)).clicked() {
                        let deleted = crate::catalog::delete_old_cache(&cache_dir, self.cache_manager_days as u64);
                        let stats = crate::catalog::cache_stats(&cache_dir);
                        self.cache_manager_stats = Some(stats);
                        self.cache_manager_result = Some(format!("{} 件のキャッシュを削除しました。", deleted));
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // ── すべて削除 ────────────────────────────────
                    if ui.button("  すべてのキャッシュを削除する  ").clicked() {
                        let deleted = crate::catalog::delete_all_cache(&cache_dir);
                        self.cache_manager_stats = Some((0, 0));
                        self.cache_manager_result = Some(format!("{} 件のキャッシュをすべて削除しました。", deleted));
                    }

                    // ── 結果メッセージ ────────────────────────────
                    if let Some(ref msg) = self.cache_manager_result {
                        ui.add_space(8.0);
                        ui.label(msg.as_str());
                    }
                });

            if !open {
                self.show_cache_manager = false;
            }
        }

        // ── キャッシュ作成ポップアップ ────────────────────────────────
        if self.show_cache_creator {
            // 完了初回に結果メッセージをセット
            if self.cache_creator_finished.load(Ordering::Relaxed)
                && self.cache_creator_result.is_none()
            {
                let done = self.cache_creator_done.load(Ordering::Relaxed);
                let total = self.cache_creator_total.load(Ordering::Relaxed);
                let cancelled = self.cache_creator_cancel.load(Ordering::Relaxed);
                self.cache_creator_result = Some(if cancelled {
                    format!("キャンセルされました（{} / {} フォルダ処理済み）", done, total)
                } else {
                    format!("{} フォルダの処理が完了しました。", done)
                });
            }

            let mut open = true;
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
            egui::Window::new("キャッシュ作成")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(500.0);

                    if !self.cache_creator_running
                        && !self.cache_creator_finished.load(Ordering::Relaxed)
                    {
                        // ── 選択前画面 ──
                        ui.label("キャッシュを作成するお気に入りを選んでください：");
                        ui.add_space(6.0);

                        if self.settings.favorites.is_empty() {
                            ui.label(egui::RichText::new("（お気に入りが未登録です）").weak());
                        } else {
                            for (i, fav) in self.settings.favorites.iter().enumerate() {
                                let path_str = fav.to_string_lossy().to_string();
                                ui.checkbox(&mut self.cache_creator_checked[i], path_str);
                            }
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);

                        let any_checked = self.cache_creator_checked.iter().any(|&b| b);
                        if ui
                            .add_enabled(
                                any_checked,
                                egui::Button::new("  キャッシュ作成  "),
                            )
                            .clicked()
                        {
                            self.start_cache_creation();
                        }
                    } else {
                        // ── 実行中 / 完了画面 ──
                        let counting = self.cache_creator_counting.load(Ordering::Relaxed);
                        let total = self.cache_creator_total.load(Ordering::Relaxed);
                        let done = self.cache_creator_done.load(Ordering::Relaxed);
                        let size = self.cache_creator_cache_size.load(Ordering::Relaxed);

                        if counting {
                            ui.label("フォルダを列挙中…");
                        } else {
                            ui.label(format!("フォルダ: {} / {}", done, total));
                        }

                        let current = self.cache_creator_current.lock().unwrap().clone();
                        if !current.is_empty() {
                            ui.label(
                                egui::RichText::new(format!("現在: {}", current))
                                    .weak()
                                    .small(),
                            );
                        }

                        ui.add_space(4.0);
                        ui.label(format!("キャッシュ容量: {}", format_bytes(size)));

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);

                        if self.cache_creator_finished.load(Ordering::Relaxed) {
                            if let Some(ref msg) = self.cache_creator_result {
                                ui.label(msg.as_str());
                                ui.add_space(4.0);
                            }
                            if ui.button("  閉じる  ").clicked() {
                                self.show_cache_creator = false;
                                self.cache_creator_running = false;
                            }
                        } else {
                            if ui.button("  キャンセル  ").clicked() {
                                self.cache_creator_cancel.store(true, Ordering::Relaxed);
                            }
                            // リアルタイム更新のため繰り返し描画要求
                            ctx.request_repaint_after(std::time::Duration::from_millis(100));
                        }
                    }
                });

            if !open {
                if self.cache_creator_running
                    && !self.cache_creator_finished.load(Ordering::Relaxed)
                {
                    self.cache_creator_cancel.store(true, Ordering::Relaxed);
                }
                self.show_cache_creator = false;
                self.cache_creator_running = false;
            }
        }

        // ── サムネイル画質設定ポップアップ ────────────────────────────
        if self.show_thumb_quality_dialog {
            let mut open = true;
            let mut apply_a = false;
            let mut apply_b = false;
            let mut reencode_a = false;
            let mut reencode_b = false;
            let mut open_fs_a = false;
            let mut open_fs_b = false;

            // 実グリッドの現在のセルサイズを取得（最小値を確保してスライダーが入るように）
            let grid_cell_w = self.last_cell_size.max(200.0);
            let grid_cell_h = self.last_cell_h.max(150.0);
            // ダイアログのデフォルトサイズ（2カラム + パディング）
            let default_w = (grid_cell_w * 2.0 + 80.0).clamp(680.0, 1800.0);
            let default_h = (grid_cell_h + 260.0).clamp(480.0, 1200.0);
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            egui::Window::new("サムネイル画質設定")
                .open(&mut open)
                .resizable(true)
                .collapsible(false)
                .default_pos(dialog_pos)
                .default_size([default_w, default_h])
                .show(ctx, |ui| {
                    if self.tq_sample.is_none() {
                        ui.set_min_width(360.0);
                        ui.label("画像を1枚選択してからもう一度お試しください。");
                        ui.add_space(8.0);
                        if ui.button("  閉じる  ").clicked() {
                            self.show_thumb_quality_dialog = false;
                        }
                        return;
                    }

                    // サンプル画像情報
                    if let Some(ref p) = self.tq_sample_path {
                        ui.label(
                            egui::RichText::new(format!("サンプル: {}", p.to_string_lossy()))
                                .small(),
                        );
                    }
                    if let Some(ref img) = self.tq_sample {
                        let sz = self.tq_sample_original_size;
                        let sz_str = if sz >= 1024 * 1024 {
                            format!("{:.1} MB", sz as f64 / (1024.0 * 1024.0))
                        } else {
                            format!("{:.0} KB", sz as f64 / 1024.0)
                        };
                        ui.label(
                            egui::RichText::new(format!(
                                "（元サイズ {}x{} / {}）",
                                img.width(),
                                img.height(),
                                sz_str
                            ))
                            .weak()
                            .small(),
                        );
                    }

                    // 現在のグリッド表示サイズ（サイズ選択時の参考用）
                    ui.label(
                        egui::RichText::new(format!(
                            "現在のグリッド表示サイズ: {} × {} px  （{} 列 / アスペクト比 {}）",
                            self.last_cell_size.round() as i32,
                            self.last_cell_h.round() as i32,
                            self.settings.grid_cols,
                            self.settings.thumb_aspect.label(),
                        ))
                        .small(),
                    );

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── A / B 2 カラム ────────────────────────
                    ui.columns(2, |cols| {
                        // -- A --
                        cols[0].vertical(|ui| {
                            ui.heading("A");
                            ui.add_space(4.0);
                            let resp = tq_draw_preview(
                                ui,
                                &self.tq_a_texture,
                                grid_cell_w,
                                grid_cell_h,
                            );
                            if resp.clicked() {
                                open_fs_a = true;
                            }
                            ui.add_space(6.0);

                            ui.horizontal(|ui| {
                                ui.label("サイズ:");
                                let resp = ui.add(
                                    egui::Slider::new(&mut self.tq_a_size, 128..=1536)
                                        .text("px"),
                                );
                                if resp.drag_stopped() || resp.lost_focus() {
                                    reencode_a = true;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("品質:");
                                let resp = ui.add(
                                    egui::Slider::new(&mut self.tq_a_quality, 1..=100),
                                );
                                if resp.drag_stopped() || resp.lost_focus() {
                                    reencode_a = true;
                                }
                            });
                            ui.add_space(4.0);
                            ui.label(format!("{}  ({}x{})",
                                format_bytes_small(self.tq_a_bytes as u64),
                                self.tq_a_texture.as_ref().map(|t| t.size()[0]).unwrap_or(0),
                                self.tq_a_texture.as_ref().map(|t| t.size()[1]).unwrap_or(0),
                            ));
                            ui.add_space(4.0);
                            if ui.button("  A を適用  ").clicked() {
                                apply_a = true;
                            }
                        });

                        // -- B --
                        cols[1].vertical(|ui| {
                            ui.heading("B");
                            ui.add_space(4.0);
                            let resp = tq_draw_preview(
                                ui,
                                &self.tq_b_texture,
                                grid_cell_w,
                                grid_cell_h,
                            );
                            if resp.clicked() {
                                open_fs_b = true;
                            }
                            ui.add_space(6.0);

                            ui.horizontal(|ui| {
                                ui.label("サイズ:");
                                let resp = ui.add(
                                    egui::Slider::new(&mut self.tq_b_size, 128..=1536)
                                        .text("px"),
                                );
                                if resp.drag_stopped() || resp.lost_focus() {
                                    reencode_b = true;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("品質:");
                                let resp = ui.add(
                                    egui::Slider::new(&mut self.tq_b_quality, 1..=100),
                                );
                                if resp.drag_stopped() || resp.lost_focus() {
                                    reencode_b = true;
                                }
                            });
                            ui.add_space(4.0);
                            ui.label(format!("{}  ({}x{})",
                                format_bytes_small(self.tq_b_bytes as u64),
                                self.tq_b_texture.as_ref().map(|t| t.size()[0]).unwrap_or(0),
                                self.tq_b_texture.as_ref().map(|t| t.size()[1]).unwrap_or(0),
                            ));
                            ui.add_space(4.0);
                            if ui.button("  B を適用  ").clicked() {
                                apply_b = true;
                            }
                        });
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "現在の設定: {}px / q={}",
                            self.settings.thumb_px, self.settings.thumb_quality
                        ));
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.button("  閉じる  ").clicked() {
                                    self.show_thumb_quality_dialog = false;
                                }
                            },
                        );
                    });
                });

            if reencode_a {
                self.reencode_tq_panel(ctx, true);
            }
            if reencode_b {
                self.reencode_tq_panel(ctx, false);
            }
            if open_fs_a || open_fs_b {
                self.tq_fullscreen = true;
                // divider 位置はリセットせず、前回の位置を維持する
            }
            if apply_a {
                self.settings.thumb_px = self.tq_a_size;
                self.settings.thumb_quality = self.tq_a_quality;
                self.settings.save();
                self.close_thumb_quality_dialog();
            } else if apply_b {
                self.settings.thumb_px = self.tq_b_size;
                self.settings.thumb_quality = self.tq_b_quality;
                self.settings.save();
                self.close_thumb_quality_dialog();
            } else if !open {
                self.close_thumb_quality_dialog();
            }
        }

        // ── サムネイル画質プレビュー全画面 A/B 比較オーバーレイ ────────
        if self.tq_fullscreen {
            let screen = ctx.content_rect();

            // A・B のテクスチャ。両方とも同じソース画像から作られたサムネイルなので
            // アスペクト比は同一。どちらかのサイズで fit 計算する。
            let ref_size = self
                .tq_a_texture
                .as_ref()
                .map(|t| t.size_vec2())
                .or_else(|| self.tq_b_texture.as_ref().map(|t| t.size_vec2()));

            // 画像表示領域を画面中央に計算（下部に情報バー分のスペースを確保）
            let img_rect_opt: Option<egui::Rect> = ref_size.map(|rs| {
                let margin = 40.0;
                let info_bar_h = 80.0;
                let avail_w = (screen.width() - margin * 2.0).max(1.0);
                let avail_h = (screen.height() - margin * 2.0 - info_bar_h).max(1.0);
                let scale = (avail_w / rs.x).min(avail_h / rs.y);
                let img_size = rs * scale;
                egui::Rect::from_center_size(
                    egui::pos2(screen.center().x, screen.center().y - info_bar_h * 0.5),
                    img_size,
                )
            });

            let divider_t = self.tq_fs_divider.clamp(0.0, 1.0);

            let area_resp = egui::Area::new(egui::Id::new("tq_fs_overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(screen.min)
                .show(ctx, |ui| {
                    let (rect, response) = ui.allocate_exact_size(
                        screen.size(),
                        egui::Sense::click_and_drag(),
                    );
                    let painter = ui.painter();
                    // 背景
                    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(20, 20, 20));

                    let Some(img_rect) = img_rect_opt else {
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "プレビューがありません",
                            egui::FontId::proportional(18.0),
                            egui::Color32::from_gray(200),
                        );
                        return response;
                    };

                    let divider_x = img_rect.min.x + img_rect.width() * divider_t;

                    // A (左側) を divider まで描画
                    if let Some(ta) = &self.tq_a_texture {
                        let a_rect = egui::Rect::from_min_max(
                            img_rect.min,
                            egui::pos2(divider_x, img_rect.max.y),
                        );
                        let a_uv = egui::Rect::from_min_max(
                            egui::pos2(0.0, 0.0),
                            egui::pos2(divider_t, 1.0),
                        );
                        if a_rect.width() > 0.0 {
                            painter.image(ta.id(), a_rect, a_uv, egui::Color32::WHITE);
                        }
                    }

                    // B (右側) を divider から描画
                    if let Some(tb) = &self.tq_b_texture {
                        let b_rect = egui::Rect::from_min_max(
                            egui::pos2(divider_x, img_rect.min.y),
                            img_rect.max,
                        );
                        let b_uv = egui::Rect::from_min_max(
                            egui::pos2(divider_t, 0.0),
                            egui::pos2(1.0, 1.0),
                        );
                        if b_rect.width() > 0.0 {
                            painter.image(tb.id(), b_rect, b_uv, egui::Color32::WHITE);
                        }
                    }

                    // 画像の外枠
                    painter.rect_stroke(
                        img_rect,
                        0.0,
                        egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
                        egui::StrokeKind::Outside,
                    );

                    // 縦境界線
                    painter.line_segment(
                        [
                            egui::pos2(divider_x, img_rect.min.y),
                            egui::pos2(divider_x, img_rect.max.y),
                        ],
                        egui::Stroke::new(2.0, egui::Color32::WHITE),
                    );

                    // ドラッグハンドル（円 + 左右矢印）
                    let handle_center = egui::pos2(divider_x, img_rect.center().y);
                    let handle_r = 16.0;
                    painter.circle_filled(
                        handle_center,
                        handle_r,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 230),
                    );
                    painter.circle_stroke(
                        handle_center,
                        handle_r,
                        egui::Stroke::new(2.0, egui::Color32::from_gray(60)),
                    );
                    painter.text(
                        handle_center,
                        egui::Align2::CENTER_CENTER,
                        "◀ ▶",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_gray(40),
                    );

                    // A / B ラベル（画像の角に半透明背景付き）
                    let label_pad = egui::vec2(10.0, 6.0);
                    let label_a = "A";
                    let label_b = "B";
                    let font = egui::FontId::proportional(24.0);

                    // A ラベル（左上、divider より左にあるときのみ）
                    if divider_t > 0.05 {
                        let pos = egui::pos2(img_rect.min.x + 12.0, img_rect.min.y + 12.0);
                        let galley = painter.layout_no_wrap(
                            label_a.to_string(),
                            font.clone(),
                            egui::Color32::WHITE,
                        );
                        let bg_rect = egui::Rect::from_min_size(
                            pos,
                            galley.size() + label_pad * 2.0,
                        );
                        painter.rect_filled(
                            bg_rect,
                            4.0,
                            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                        );
                        painter.galley(pos + label_pad, galley, egui::Color32::WHITE);
                    }

                    // B ラベル（右上、divider より右にあるときのみ）
                    if divider_t < 0.95 {
                        let galley = painter.layout_no_wrap(
                            label_b.to_string(),
                            font.clone(),
                            egui::Color32::WHITE,
                        );
                        let bg_size = galley.size() + label_pad * 2.0;
                        let pos = egui::pos2(
                            img_rect.max.x - 12.0 - bg_size.x,
                            img_rect.min.y + 12.0,
                        );
                        let bg_rect = egui::Rect::from_min_size(pos, bg_size);
                        painter.rect_filled(
                            bg_rect,
                            4.0,
                            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                        );
                        painter.galley(pos + label_pad, galley, egui::Color32::WHITE);
                    }

                    // 情報バー（画像下）
                    let info_base_y = img_rect.max.y + 24.0;
                    let a_info = format!(
                        "A:  {}px  /  q={}  /  {}",
                        self.tq_a_size,
                        self.tq_a_quality,
                        format_bytes_small(self.tq_a_bytes as u64),
                    );
                    let b_info = format!(
                        "B:  {}px  /  q={}  /  {}",
                        self.tq_b_size,
                        self.tq_b_quality,
                        format_bytes_small(self.tq_b_bytes as u64),
                    );
                    let info_font = egui::FontId::proportional(14.0);
                    painter.text(
                        egui::pos2(rect.center().x - 24.0, info_base_y),
                        egui::Align2::RIGHT_CENTER,
                        a_info,
                        info_font.clone(),
                        egui::Color32::from_rgb(150, 200, 255),
                    );
                    painter.text(
                        egui::pos2(rect.center().x + 24.0, info_base_y),
                        egui::Align2::LEFT_CENTER,
                        b_info,
                        info_font.clone(),
                        egui::Color32::from_rgb(255, 220, 150),
                    );
                    painter.text(
                        egui::pos2(rect.center().x, info_base_y + 24.0),
                        egui::Align2::CENTER_CENTER,
                        "ドラッグで境界線を移動  /  クリック または ESC で戻る",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_gray(180),
                    );

                    response
                });

            // ドラッグ → divider を更新
            if area_resp.inner.dragged() {
                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                    if let Some(img_rect) = img_rect_opt {
                        if img_rect.width() > 0.0 {
                            let t = ((pos.x - img_rect.min.x) / img_rect.width())
                                .clamp(0.0, 1.0);
                            self.tq_fs_divider = t;
                            ctx.request_repaint();
                        }
                    }
                }
            }

            // 画像上にホバーしているときはリサイズ左右カーソル
            if let Some(img_rect) = img_rect_opt {
                if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                    if img_rect.contains(pos) {
                        ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                }
            }

            // ドラッグしていないクリック → 閉じる
            let clicked = area_resp.inner.clicked();
            let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            if clicked || esc {
                self.tq_fullscreen = false;
            }
        }

        // ── 環境設定ポップアップ ─────────────────────────────────────
        if self.show_preferences {
            let mut open = true;
            let mut apply = false;
            let mut cancel = false;
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            egui::Window::new("環境設定")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(420.0);

                    ui.heading("並列読み込み");
                    ui.add_space(4.0);

                    let is_auto = self.settings.parallelism == crate::settings::Parallelism::Auto;
                    let auto_count = {
                        let cores = std::thread::available_parallelism()
                            .map(|n| n.get()).unwrap_or(2);
                        (cores / 2).max(1)
                    };

                    if ui.radio(is_auto, format!("自動（CPUコア数の半分: {} スレッド）", auto_count)).clicked() {
                        self.settings.parallelism = crate::settings::Parallelism::Auto;
                    }

                    ui.horizontal(|ui| {
                        if ui.radio(!is_auto, "手動").clicked() {
                            self.settings.parallelism =
                                crate::settings::Parallelism::Manual(self.pref_manual_threads);
                        }
                        ui.add_enabled(
                            !is_auto,
                            egui::DragValue::new(&mut self.pref_manual_threads)
                                .range(1..=64)
                                .suffix(" スレッド"),
                        );
                        if !is_auto {
                            self.settings.parallelism =
                                crate::settings::Parallelism::Manual(self.pref_manual_threads);
                        }
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    ui.heading("フルサイズ画像の先読み");
                    ui.add_space(4.0);
                    ui.label("フルサイズ表示時に前後の画像を先読みする枚数（各最大 50 枚）。");
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label("後方（前の画像）:");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.prefetch_back)
                                .range(0..=50usize)
                                .suffix(" 枚"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("前方（次の画像）:");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.prefetch_forward)
                                .range(0..=50usize)
                                .suffix(" 枚"),
                        );
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    ui.heading("サムネイルの先読み");
                    ui.add_space(4.0);
                    ui.label(
                        "サムネイルグリッドで現在位置の前後に何ページ分を GPU に保持するか。\n\
                         範囲外はメモリから破棄され、スクロールで戻ると再読み込みされます。",
                    );
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label("後方（前のページ）:");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.thumb_prev_pages)
                                .range(0..=20u32)
                                .suffix(" ページ"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("前方（次のページ）:");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.thumb_next_pages)
                                .range(0..=20u32)
                                .suffix(" ページ"),
                        );
                    });

                    ui.add_space(6.0);
                    // プライマリ GPU の VRAM を問い合わせて表示に使う
                    let vram_mib = crate::gpu_info::query_vram_summary_mib();
                    let vram_label = match vram_mib {
                        Some(mib) if mib >= 1024 => {
                            format!("{:.1} GiB", mib as f64 / 1024.0)
                        }
                        Some(mib) => format!("{} MiB", mib),
                        None => "取得失敗 (4 GiB 仮定)".to_string(),
                    };
                    ui.label(format!(
                        "GPU メモリ上限 (安全ネット):\n\
                         超過時は先読み範囲を自動的に縮小します。\n\
                         検出した GPU の VRAM: {vram_label}",
                    ));

                    ui.horizontal(|ui| {
                        ui.label("上限:");
                        ui.add(
                            egui::Slider::new(
                                &mut self.settings.thumb_vram_cap_percent,
                                0..=100u32,
                            )
                            .step_by(5.0)
                            .suffix(" %"),
                        );
                    });

                    // 現在の % が実際に何 MiB に相当するかを補助表示
                    {
                        let pct = self.settings.thumb_vram_cap_percent;
                        let text = if pct == 0 {
                            "  ↑ 0% = 無制限 (推奨しない)".to_string()
                        } else {
                            let cap_mib = crate::gpu_info::vram_cap_from_percent(pct)
                                / (1024 * 1024);
                            format!(
                                "  ↑ VRAM の {}% = 約 {} MiB を上限とします (推奨: 50%)",
                                pct, cap_mib
                            )
                        };
                        ui.label(text);
                    }

                    ui.add_space(6.0);
                    ui.checkbox(
                        &mut self.settings.thumb_idle_upgrade,
                        "アイドル時にキャッシュ由来のサムネイルを高画質化する",
                    );
                    ui.label(
                        "  ↑ スクロール停止後、キャッシュ復元 (WebP q=75) のサムネイルを\n    \
                         元画像から再デコードして差し替えます。visible 側から順次処理。",
                    );

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    ui.heading("フォルダ移動");
                    ui.add_space(4.0);
                    ui.label("Ctrl+↑↓ で移動先フォルダに画像がない場合、自動でスキップする最大回数。");
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("空フォルダのスキップ上限:");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.folder_skip_limit)
                                .range(1..=10usize)
                                .suffix(" 回"),
                        );
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("  OK  ").clicked() {
                            apply = true;
                        }
                        if ui.button("キャンセル").clicked() {
                            cancel = true;
                        }
                    });
                });

            if apply {
                self.settings.save();
                self.show_preferences = false;
            } else if cancel || !open {
                // キャンセル/×ボタン: 変更を破棄するため再ロード
                self.settings = crate::settings::Settings::load();
                self.show_preferences = false;
            }
        }

        // ── キャッシュ生成設定ポップアップ (段階 C) ─────────────────────
        if self.show_cache_policy_dialog {
            let mut open = true;
            let mut apply = false;
            let mut cancel = false;
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            egui::Window::new("キャッシュ生成設定")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(480.0);

                    ui.label(
                        "サムネイルキャッシュをいつ生成するかを指定します。\n\
                         リリースビルドではキャッシュが無くても十分高速ですが、\n\
                         重い画像や巨大ファイルはキャッシュすると再訪問時に高速化します。\n\
                         Off にしても既存のキャッシュは引き続き読み込まれます。",
                    );
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    ui.heading("モード");
                    ui.add_space(4.0);
                    for policy in [
                        crate::settings::CachePolicy::Off,
                        crate::settings::CachePolicy::Auto,
                        crate::settings::CachePolicy::Always,
                    ] {
                        if ui
                            .radio(
                                self.settings.cache_policy == policy,
                                policy.label(),
                            )
                            .clicked()
                        {
                            self.settings.cache_policy = policy;
                        }
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // Auto モード時のみ以下の項目を活性化
                    let auto_active =
                        self.settings.cache_policy == crate::settings::CachePolicy::Auto;

                    ui.add_enabled_ui(auto_active, |ui| {
                        ui.heading("Auto モードのしきい値");
                        ui.add_space(4.0);

                        ui.label("時間しきい値 (decode + display の合計がこれ以上ならキャッシュ):");
                        ui.add(
                            egui::Slider::new(
                                &mut self.settings.cache_threshold_ms,
                                10..=100,
                            )
                            .step_by(5.0)
                            .suffix(" ms"),
                        );
                        ui.label("  小さいほど多くキャッシュ。25 ms 推奨。");

                        ui.add_space(8.0);

                        // サイズしきい値を MB 単位で編集
                        ui.label("サイズしきい値 (このサイズ以上は無条件キャッシュ):");
                        let mut size_mb =
                            (self.settings.cache_size_threshold_bytes as f64) / 1_000_000.0;
                        if ui
                            .add(
                                egui::Slider::new(&mut size_mb, 0.5..=10.0)
                                    .step_by(0.5)
                                    .suffix(" MB"),
                            )
                            .changed()
                        {
                            self.settings.cache_size_threshold_bytes =
                                (size_mb * 1_000_000.0) as u64;
                        }
                        ui.label("  2 MB 推奨。これ以上の重い画像が確実にキャッシュされます。");

                        ui.add_space(8.0);

                        ui.checkbox(
                            &mut self.settings.cache_webp_always,
                            "既存 .webp は常にキャッシュ (デコードが重いため推奨)",
                        );
                    });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("  OK  ").clicked() {
                            apply = true;
                        }
                        if ui.button("キャンセル").clicked() {
                            cancel = true;
                        }
                    });
                });

            if apply {
                self.settings.save();
                self.show_cache_policy_dialog = false;
            } else if cancel || !open {
                // キャンセル/×ボタン: 変更を破棄するため再ロード
                self.settings = crate::settings::Settings::load();
                self.show_cache_policy_dialog = false;
            }
        }

        // ── 統計ダイアログ ──────────────────────────────────────────
        if self.show_stats_dialog {
            let mut open = true;
            let mut reset_clicked = false;
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            // スナップショットを取得 (ロック時間を最小化)
            let snapshot: crate::stats::ThumbStats = {
                self.stats.lock().map(|s| s.clone()).unwrap_or_default()
            };

            egui::Window::new("統計")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(520.0);
                    ui.label(
                        "起動時から累計したサムネイル読み込み統計です。\n\
                         キャッシュ生成設定の参考にしてください。\n\
                         (キャッシュヒットは対象外。フルデコード時のみ記録)",
                    );
                    ui.add_space(8.0);

                    // ── 読み込み時間ヒストグラム ──
                    ui.heading("読み込み時間 (decode + display)");
                    ui.add_space(4.0);
                    draw_histogram(
                        ui,
                        &snapshot.load_time_hist,
                        |bucket| crate::stats::ThumbStats::load_time_label(bucket),
                    );

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── ファイルサイズヒストグラム ──
                    ui.heading("ファイルサイズ");
                    ui.add_space(4.0);
                    draw_histogram(
                        ui,
                        &snapshot.size_hist,
                        |bucket| crate::stats::ThumbStats::size_label(bucket),
                    );

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── フォーマット別件数 ──
                    ui.heading("フォーマット");
                    ui.add_space(4.0);
                    let format_rows: [(&str, u64); 7] = [
                        ("JPEG  ", snapshot.count_jpg),
                        ("PNG   ", snapshot.count_png),
                        ("WebP  ", snapshot.count_webp),
                        ("GIF   ", snapshot.count_gif),
                        ("BMP   ", snapshot.count_bmp),
                        ("動画  ", snapshot.count_video),
                        ("その他", snapshot.count_other),
                    ];
                    draw_format_rows(ui, &format_rows);

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── サマリ ──
                    let total_images = snapshot.total_images();
                    let total_all = total_images + snapshot.count_video;
                    ui.label(format!(
                        "合計: {} 件  (画像 {} / 動画 {} / 失敗 {})",
                        format_count(total_all),
                        format_count(total_images),
                        format_count(snapshot.count_video),
                        format_count(snapshot.count_failed),
                    ));

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("リセット").clicked() {
                            reset_clicked = true;
                        }
                        if ui.button("閉じる").clicked() {
                            // open = false でダイアログを閉じる
                        }
                    });
                });

            if reset_clicked {
                if let Ok(mut s) = self.stats.lock() {
                    s.reset();
                }
            }
            if !open {
                self.show_stats_dialog = false;
            }
        }

        // ── ツールバー（列数・アスペクト比・ソート順の即時切り替え）──
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("列:");
                for cols in 2..=10usize {
                    let selected = self.settings.grid_cols == cols;
                    if ui.selectable_label(selected, format!(" {cols} ")).clicked() {
                        self.settings.grid_cols = cols;
                        self.settings.save();
                    }
                }
                ui.separator();
                ui.label("比率:");
                for &aspect in crate::settings::ThumbAspect::all() {
                    let selected = self.settings.thumb_aspect == aspect;
                    if ui.selectable_label(selected, aspect.label()).clicked() {
                        self.settings.thumb_aspect = aspect;
                        self.settings.save();
                    }
                }
                ui.separator();
                ui.label("ソート:");
                for &order in crate::settings::SortOrder::all() {
                    let selected = self.settings.sort_order == order;
                    if ui.selectable_label(selected, order.short_label()).clicked()
                        && !selected
                    {
                        self.settings.sort_order = order;
                        self.settings.save();
                        if let Some(path) = self.current_folder.clone() {
                            self.folder_history.remove(&path);
                            self.load_folder(path);
                        }
                    }
                }
            });
            ui.add_space(2.0);
        });

        // ── アドレスバー ─────────────────────────────────────────────
        let address_nav = egui::TopBottomPanel::top("address_bar")
            .show(ctx, |ui| -> Option<PathBuf> {
                ui.add_space(3.0);
                let mut result = None;
                ui.horizontal(|ui| {
                    ui.label("フォルダ:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.address)
                            .desired_width(f32::INFINITY),
                    );
                    self.address_has_focus = resp.has_focus();
                    if resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let p = PathBuf::from(&self.address);
                        if p.is_dir() {
                            result = Some(p);
                        }
                    }
                });
                ui.add_space(3.0);
                result
            })
            .inner;

        // ── サムネイルグリッド ────────────────────────────────────────
        let scroll_to = self.scroll_to_selected;
        self.scroll_to_selected = false;

        let grid_nav = egui::CentralPanel::default()
            .show(ctx, |ui| -> Option<PathBuf> {
                if self.items.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label("フォルダを入力して Enter キーを押してください");
                    });
                    return None;
                }

                let cols = self.settings.grid_cols.max(1);
                let avail_w = ui.available_width();
                let cell_w = (avail_w / cols as f32).floor();
                let cell_h = (cell_w * self.settings.thumb_aspect.height_ratio()).round().max(1.0);

                // ウィンドウリサイズやアスペクト比変更でセルサイズが変わった場合スナップし直す
                if (cell_w - self.last_cell_size).abs() > 0.5
                    || (cell_h - self.last_cell_h).abs() > 0.5
                {
                    self.scroll_offset_y =
                        (self.scroll_offset_y / cell_h).round() * cell_h;
                    self.last_cell_size = cell_w;
                    self.last_cell_h = cell_h;
                }

                if scroll_to {
                    self.apply_scroll_to_selected(cols, cell_h);
                }

                let total_rows = self.items.len().div_ceil(cols);
                let total_h = total_rows as f32 * cell_h;

                // スクロール上限（行境界にスナップ済み）
                let max_offset = if total_h <= self.last_viewport_h {
                    0.0
                } else {
                    (((total_h - self.last_viewport_h) / cell_h).ceil() * cell_h)
                        .min(total_h)
                };
                self.scroll_offset_y = self.scroll_offset_y.min(max_offset);

                let mut nav: Option<PathBuf> = None;

                // egui にスクロールを管理させず、自前の offset を毎フレーム注入する
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .vertical_scroll_offset(self.scroll_offset_y)
                    .show_viewport(ui, |ui, viewport| {
                        // ビューポート高さを記録（次フレームのスクロール計算に使う）
                        self.last_viewport_h = viewport.height();

                        let (content_rect, _) = ui.allocate_exact_size(
                            egui::vec2(avail_w, total_h),
                            egui::Sense::hover(),
                        );

                        let first_row = (viewport.min.y / cell_h) as usize;
                        let last_row =
                            ((viewport.max.y / cell_h) as usize + 2).min(total_rows);

                        // Phase 2b ワーカーへ現在の可視先頭アイテムを通知
                        // ワーカーはこの値に最も近いアイテムを優先してデコードする
                        self.scroll_hint.store(first_row * cols, Ordering::Relaxed);

                        for row in first_row..last_row {
                            for col in 0..cols {
                                let idx = row * cols + col;
                                if idx >= self.items.len() {
                                    break;
                                }

                                let cell_rect = egui::Rect::from_min_size(
                                    content_rect.min
                                        + egui::vec2(
                                            col as f32 * cell_w,
                                            row as f32 * cell_h,
                                        ),
                                    egui::vec2(cell_w, cell_h),
                                );

                                let response = ui.interact(
                                    cell_rect,
                                    ui.id().with(idx),
                                    egui::Sense::click(),
                                );
                                if response.clicked() {
                                    self.selected = Some(idx);
                                    self.update_last_selected_image();
                                }
                                if response.double_clicked() {
                                    match self.items.get(idx) {
                                        Some(GridItem::Folder(p)) => nav = Some(p.clone()),
                                        Some(GridItem::Image(_)) => self.open_fullscreen(idx),
                                        Some(GridItem::Video(p)) => {
                                            let vp = p.clone();
                                            open_external_player(&vp);
                                        }
                                        None => {}
                                    }
                                }

                                draw_cell(
                                    ui,
                                    cell_rect,
                                    self.selected == Some(idx),
                                    &self.items[idx],
                                    &self.thumbnails[idx],
                                );
                            }
                        }
                    });

                nav
            })
            .inner;

        let navigate = fav_nav.or(keyboard_nav).or(address_nav).or(grid_nav);
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

fn draw_cell(
    ui: &egui::Ui,
    rect: egui::Rect,
    is_selected: bool,
    item: &GridItem,
    thumb: &ThumbnailState,
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
        GridItem::Image(_) => match thumb {
            ThumbnailState::Loaded { tex, .. } => {
                let tex_size = tex.size_vec2();
                let scale =
                    (inner.width() / tex_size.x).min(inner.height() / tex_size.y);
                let img_size = tex_size * scale;
                let img_rect = egui::Rect::from_center_size(inner.center(), img_size);
                painter.image(
                    tex.id(),
                    img_rect,
                    egui::Rect::from_min_max(
                        egui::pos2(0.0, 0.0),
                        egui::pos2(1.0, 1.0),
                    ),
                    egui::Color32::WHITE,
                );
            }
            ThumbnailState::Pending | ThumbnailState::Evicted => {
                // 段階 B: Evicted は「一度ロードしたが破棄された」状態だが
                // 表示上は Pending と同じプレースホルダを描く (再ロード待ち)
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
        },
        GridItem::Video(path) => {
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    let tex_size = tex.size_vec2();
                    let scale = (inner.width() / tex_size.x).min(inner.height() / tex_size.y);
                    let img_rect = egui::Rect::from_center_size(inner.center(), tex_size * scale);
                    painter.image(
                        tex.id(),
                        img_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                ThumbnailState::Pending | ThumbnailState::Evicted => {
                    // 動画は keep_range ロジックの対象外だが、ThumbnailState 自体は共有なので
                    // Evicted になる可能性もある (update_keep_range_and_requests でスキップ)
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
    }

    let border = if is_selected {
        egui::Stroke::new(2.0, egui::Color32::from_rgb(60, 120, 220))
    } else {
        egui::Stroke::new(1.0, egui::Color32::from_gray(200))
    };
    painter.rect_stroke(rect, 2.0, border, egui::StrokeKind::Middle);
}

// -----------------------------------------------------------------------
// アニメーション GIF / APNG デコード
// -----------------------------------------------------------------------

/// GIF をデコードしてアニメーションフレーム列を返す。
/// 静止画（1フレーム）や失敗時は None を返す。
fn decode_gif_frames(path: &Path) -> Option<Vec<(egui::ColorImage, f64)>> {
    use image::codecs::gif::GifDecoder;
    use image::AnimationDecoder;

    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let decoder = GifDecoder::new(reader).ok()?;
    let frames = decoder.into_frames().collect_frames().ok()?;
    if frames.len() <= 1 { return None; }

    Some(frames.into_iter().map(|frame| {
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay = if denom > 0 { numer as f64 / denom as f64 / 1000.0 } else { 0.1 };
        let delay = delay.max(0.02); // 最低 20ms（Chrome 互換）
        let buf = frame.into_buffer();
        let (w, h) = buf.dimensions();
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [w as usize, h as usize], buf.as_raw(),
        );
        (ci, delay)
    }).collect())
}

/// APNG をデコードしてアニメーションフレーム列を返す。
/// 静止画（1フレーム）・非 APNG・失敗時は None を返す。
fn decode_apng_frames(path: &Path) -> Option<Vec<(egui::ColorImage, f64)>> {
    use image::codecs::png::PngDecoder;
    use image::AnimationDecoder;

    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let decoder = PngDecoder::new(reader).ok()?;
    if !decoder.is_apng().ok()? { return None; }

    let frames = decoder.apng().ok()?.into_frames().collect_frames().ok()?;
    if frames.len() <= 1 { return None; }

    Some(frames.into_iter().map(|frame| {
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay = if denom > 0 { numer as f64 / denom as f64 / 1000.0 } else { 0.1 };
        let delay = delay.max(0.02);
        let buf = frame.into_buffer();
        let (w, h) = buf.dimensions();
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [w as usize, h as usize], buf.as_raw(),
        );
        (ci, delay)
    }).collect())
}

/// サムネイル読み込み結果メッセージ。
///
/// `(item_idx, Option<ColorImage>, from_cache)`
/// - `from_cache = true`: WebP キャッシュから復元 (段階 E アップグレード対象)
/// - `from_cache = false`: 元画像から直接デコード (高画質) または動画 Shell API
type ThumbMsg = (usize, Option<egui::ColorImage>, bool);

/// 段階 B: サムネイル読み込み要求。
///
/// UI スレッドが `reload_queue` に push し、永続ワーカースレッドが pop して処理する。
/// ワーカーはまず `cache_map` を参照し、ヒットすれば WebP デコード、
/// ミスすれば `load_one_cached` に委譲する。
struct LoadRequest {
    idx: usize,
    path: PathBuf,
    mtime: i64,
    file_size: i64,
    /// 段階 E: true の場合はキャッシュを無視して元画像から再デコードする
    skip_cache: bool,
}

/// キャッシュ生成判定用のパラメータ（段階 C）。
///
/// Settings から必要なフィールドのみを抽出した Copy 可能な構造体で、
/// 複数スレッドへ安価に配布できる。
#[derive(Clone, Copy)]
struct CacheDecision {
    policy: crate::settings::CachePolicy,
    threshold_ms: u32,
    size_threshold: u64,
    webp_always: bool,
    // cache_videos_always は動画が別パス (video_thumb) を通るため load_one_cached では使わない
}

impl CacheDecision {
    fn from_settings(s: &crate::settings::Settings) -> Self {
        Self {
            policy: s.cache_policy,
            threshold_ms: s.cache_threshold_ms,
            size_threshold: s.cache_size_threshold_bytes,
            webp_always: s.cache_webp_always,
        }
    }

    /// 指定画像をキャッシュに保存すべきか判定する。
    ///
    /// - `Always`: 常に true
    /// - `Off`   : 常に false
    /// - `Auto`  : 事前ヒューリスティック (ext==webp / サイズ) または
    ///             実測時間 (decode_ms + display_ms) がしきい値以上
    fn should_cache(
        &self,
        path: &Path,
        file_size: i64,
        decode_ms: f64,
        display_ms: f64,
    ) -> bool {
        use crate::settings::CachePolicy;
        match self.policy {
            CachePolicy::Always => true,
            CachePolicy::Off    => false,
            CachePolicy::Auto   => {
                // 事前ヒューリスティック
                if self.webp_always {
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|s| s.to_lowercase())
                        .unwrap_or_default();
                    if ext == "webp" {
                        return true;
                    }
                }
                if (file_size as u64) >= self.size_threshold {
                    return true;
                }
                // 実測判定
                (decode_ms + display_ms) >= self.threshold_ms as f64
            }
        }
    }
}

/// DynamicImage を `display_px` 以下に収まるよう Lanczos3 でリサイズし、
/// egui::ColorImage に変換する。
///
/// 表示用パス (段階 A) で使用。WebP 量子化を通さず元画像から直接生成するため
/// 画質劣化が無く、キャッシュの WebP(q=75) より高品質。
fn resize_to_display_color_image(
    img: &image::DynamicImage,
    display_px: u32,
) -> egui::ColorImage {
    let resized = img.resize(
        display_px,
        display_px,
        image::imageops::FilterType::Lanczos3,
    );
    let rgba = resized.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

/// 現在のセルサイズから表示用 ColorImage の画素数を算出する。
///
/// 論理ピクセル × DPI スケールで物理ピクセルを求め、256-2048 px にクランプする。
/// - 下限 256: 起動直後で cell_size が小さすぎる場合の最低品質保証
/// - 上限 2048: 4K 2列などの巨大セルで過大メモリを防ぐ (最大 16 MB/ColorImage)
fn compute_display_px(cell_w: f32, cell_h: f32, dpi: f32) -> u32 {
    let logical_max = cell_w.max(cell_h).max(1.0);
    let physical = (logical_max * dpi.max(0.5)).ceil();
    (physical as u32).clamp(256, 2048)
}

/// 段階 B: 1 つの `LoadRequest` を処理する。
///
/// - 通常: `cache_map` を参照しキャッシュヒットしていれば WebP を復号して送信する
///   (`from_cache = true`)
/// - ミスまたは `req.skip_cache = true`: `load_one_cached` に委譲してフルデコード
///   (`from_cache = false`、段階 E のアップグレード経路)
#[allow(clippy::too_many_arguments)]
fn process_load_request(
    req: &LoadRequest,
    cache_map: &std::collections::HashMap<String, crate::catalog::CacheEntry>,
    tx: &mpsc::Sender<ThumbMsg>,
    catalog: Option<&crate::catalog::CatalogDb>,
    thumb_px: u32,
    thumb_quality: u8,
    display_px: u32,
    cache_decision: CacheDecision,
    gen_done: &Arc<AtomicUsize>,
    stats: &Arc<Mutex<crate::stats::ThumbStats>>,
) {
    let filename = req
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    // 段階 E: skip_cache = true のときはキャッシュヒット判定を飛ばして
    // 必ず元画像からデコードする (アイドル時の画質アップグレード用)
    if !req.skip_cache {
        if let Some(entry) = cache_map.get(filename) {
            if entry.mtime == req.mtime && entry.file_size == req.file_size {
                let ci = crate::catalog::decode_thumb_to_color_image(&entry.jpeg_data);
                // from_cache = true: アップグレード対象
                let _ = tx.send((req.idx, ci, true));
                gen_done.fetch_add(1, Ordering::Relaxed);
                // 統計には記録しない: キャッシュヒットは 2-3 ms で
                // "キャッシュ無し時のコスト" を歪めるため
                return;
            }
        }
    }

    // キャッシュミス or skip_cache: フルデコード (+ 必要なら保存)
    // load_one_cached は from_cache = false を送信する
    load_one_cached(
        &req.path, req.idx, tx, catalog,
        req.mtime, req.file_size, gen_done,
        thumb_px, thumb_quality, display_px, cache_decision,
        stats,
    );
}

/// 1枚の画像をデコードしてサムネイルを生成し、(条件を満たせば) カタログに保存して
/// チャネルへ送信する。
/// catalog が None の場合はカタログへの保存をスキップする。
/// gen_done は処理完了時にインクリメントする進捗カウンタ。
///
/// 段階 A 以降のフロー:
/// 1. `image::open` でフルデコード
/// 2. **表示用 ColorImage を直接生成してチャネル送信** (UI を先に更新)
/// 3. 段階 C: `CacheDecision` で保存要否を判定
/// 4. 保存対象かつ catalog が指定されていれば WebP エンコード → DB 保存
///
/// 2 → 3/4 の順にすることで、UI 応答性を優先しつつキャッシュも作成する。
/// 表示は元画像から直接生成するため WebP 量子化の画質劣化が無い。
#[allow(clippy::too_many_arguments)]
fn load_one_cached(
    path: &Path,
    idx: usize,
    tx: &mpsc::Sender<ThumbMsg>,
    catalog: Option<&crate::catalog::CatalogDb>,
    mtime: i64,
    file_size: i64,
    gen_done: &Arc<AtomicUsize>,
    thumb_px: u32,
    thumb_quality: u8,
    display_px: u32,
    cache_decision: CacheDecision,
    stats: &Arc<Mutex<crate::stats::ThumbStats>>,
) {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
    let t = std::time::Instant::now();

    // 拡張子ベースのデコードを試み、失敗した場合はマジックバイトで再試行する。
    // 拡張子が間違っているファイル（例: WebP に .png）にも対応するため。
    let img_result = image::open(path).or_else(|_| {
        use std::io::BufReader;
        let f = std::fs::File::open(path)?;
        image::ImageReader::new(BufReader::new(f))
            .with_guessed_format()
            .map_err(image::ImageError::IoError)?
            .decode()
    });

    let img = match img_result {
        Ok(i) => i,
        Err(e) => {
            crate::logger::log(format!("    idx={idx:>4} FAIL {e}  {name}"));
            let _ = tx.send((idx, None, false));
            gen_done.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut s) = stats.lock() {
                s.record_failed();
            }
            return;
        }
    };
    let decode_ms = t.elapsed().as_secs_f64() * 1000.0;

    // (A) 表示用パス: 元画像から直接セルサイズにリサイズして UI へ送信
    //     WebP 量子化を経由しないため画質劣化なし、かつ WebP encode を待たない
    //     from_cache = false: 元画像由来の高画質 (段階 E アップグレード不要)
    let t_display = std::time::Instant::now();
    let display_ci = resize_to_display_color_image(&img, display_px);
    let display_ms = t_display.elapsed().as_secs_f64() * 1000.0;
    let _ = tx.send((idx, Some(display_ci), false));

    // 統計: 画像のフルデコード時間・サイズ・フォーマットを記録
    {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if let Ok(mut s) = stats.lock() {
            s.record_image(decode_ms + display_ms, file_size.max(0) as u64, ext);
        }
    }

    // (B) キャッシュ保存判定 (段階 C)
    //     catalog 未指定時は保存不可
    //     それ以外は CacheDecision の判定に従う
    let should_save = catalog.is_some()
        && cache_decision.should_cache(path, file_size, decode_ms, display_ms);

    if should_save {
        let cat = catalog.expect("should_save => catalog is Some");
        let t_enc = std::time::Instant::now();
        match crate::catalog::encode_thumb_webp(&img, thumb_px, thumb_quality as f32) {
            Some((webp_data, w, h)) => {
                let encode_ms = t_enc.elapsed().as_secs_f64() * 1000.0;
                if let Err(e) = cat.save(name, mtime, file_size, w, h, &webp_data) {
                    crate::logger::log(format!("    idx={idx:>4} catalog save: {e}"));
                }
                crate::logger::log(format!(
                    "    idx={idx:>4} decode={decode_ms:>6.1}ms display={display_ms:>5.1}ms encode={encode_ms:>5.1}ms  {name}"
                ));
            }
            None => {
                crate::logger::log(format!("    idx={idx:>4} WebP encode FAIL  {name}"));
            }
        }
    } else {
        crate::logger::log(format!(
            "    idx={idx:>4} decode={decode_ms:>6.1}ms display={display_ms:>5.1}ms (skip cache)  {name}"
        ));
    }

    // 成功・失敗を問わず完了としてカウント（タイトルバーの進捗に反映）
    gen_done.fetch_add(1, Ordering::Relaxed);
}

/// 半透明黒円 + 白三角（再生ボタン）を描画する。
/// `center` は円の中心、`radius` は円の半径。
fn draw_play_icon(painter: &egui::Painter, center: egui::Pos2, radius: f32) {
    // 背景円
    painter.circle_filled(
        center,
        radius,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160),
    );
    // 右向き三角形（ポリゴン）
    // 視覚的中心を合わせるため若干右にオフセット
    let tr = radius * 0.45;
    let cx = center.x + tr * 0.12;
    let cy = center.y;
    let points = vec![
        egui::pos2(cx - tr * 0.55, cy - tr * 0.9),  // 左上
        egui::pos2(cx - tr * 0.55, cy + tr * 0.9),  // 左下
        egui::pos2(cx + tr * 0.95, cy),              // 右頂点
    ];
    painter.add(egui::Shape::convex_polygon(
        points,
        egui::Color32::WHITE,
        egui::Stroke::NONE,
    ));
}

/// 自然順ソート用のキーを返す。
/// ファイル名を「テキスト部分」と「数字部分」に分割し、
/// 数字部分は数値として比較するので 1 < 2 < 9 < 10 < 11 となる。
fn natural_sort_key(name: &str) -> Vec<NaturalChunk> {
    let name_lower = name.to_lowercase();
    let mut chunks = Vec::new();
    let mut chars = name_lower.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut num_str = String::new();
            while chars.peek().map(|ch| ch.is_ascii_digit()).unwrap_or(false) {
                num_str.push(chars.next().unwrap());
            }
            let n: u64 = num_str.parse().unwrap_or(0);
            chunks.push(NaturalChunk::Num(n));
        } else {
            let mut text = String::new();
            while chars.peek().map(|ch| !ch.is_ascii_digit()).unwrap_or(false) {
                text.push(chars.next().unwrap());
            }
            chunks.push(NaturalChunk::Text(text));
        }
    }
    chunks
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum NaturalChunk {
    Text(String),
    Num(u64),
}

fn truncate_name(name: &str, max_chars: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max_chars {
        name.to_owned()
    } else {
        chars[..max_chars - 1].iter().collect::<String>() + "…"
    }
}

// -----------------------------------------------------------------------
// フォルダツリー走査（深さ優先・前順）
// -----------------------------------------------------------------------

/// フォルダ内に対応画像ファイルが1枚以上あるか確認する
fn folder_has_images(path: &std::path::Path) -> bool {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| SUPPORTED_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
                .unwrap_or(false)
        })
}

/// Ctrl+↑↓ フォルダ移動：画像なしフォルダを最大 skip_limit 回スキップする。
/// skip_limit 回以内に画像ありフォルダが見つかればそこへ移動。
/// 見つからなければ直近の隣フォルダ（1ステップ先）へ移動。
fn navigate_folder_with_skip<F>(start: &std::path::Path, nav_fn: F, skip_limit: usize) -> Option<PathBuf>
where
    F: Fn(&std::path::Path) -> Option<PathBuf>,
{
    let first = nav_fn(start)?;
    let mut candidate = first.clone();
    for _ in 0..skip_limit {
        if folder_has_images(&candidate) {
            return Some(candidate);
        }
        match nav_fn(&candidate) {
            Some(next) => candidate = next,
            None => return Some(first),
        }
    }
    // skip_limit 回分全て画像なし → 直近の隣フォルダにフォールバック
    Some(first)
}

/// 深さ優先前順で次のフォルダを返す
/// 子があれば最初の子、なければ次の兄弟、なければ祖先の次の兄弟
fn next_folder_dfs(current: &std::path::Path) -> Option<PathBuf> {
    // 1. 子フォルダがあれば最初の子へ
    if let Some(first_child) = sorted_subdirs(current).into_iter().next() {
        return Some(first_child);
    }
    // 2. 子がなければ、次の兄弟または祖先の次の兄弟を探す
    next_sibling_or_ancestor_sibling(current)
}

/// 深さ優先前順で前のフォルダを返す
/// 前の兄弟がいればその最後の子孫、最初の子であれば親
fn prev_folder_dfs(current: &std::path::Path) -> Option<PathBuf> {
    let parent = current.parent()?;
    let siblings = sorted_subdirs(parent);
    let pos = siblings.iter().position(|s| path_eq(s, current))?;

    if pos == 0 {
        // 最初の子 → 親へ
        Some(parent.to_path_buf())
    } else {
        // 前の兄弟の最後の子孫へ
        Some(last_descendant_dir(&siblings[pos - 1]))
    }
}

/// path の次の兄弟を返す。兄弟がなければ親で再帰する
fn next_sibling_or_ancestor_sibling(path: &std::path::Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let siblings = sorted_subdirs(parent);
    let pos = siblings.iter().position(|s| path_eq(s, path))?;

    if pos + 1 < siblings.len() {
        Some(siblings[pos + 1].clone())
    } else {
        next_sibling_or_ancestor_sibling(parent)
    }
}

/// path の最も深い最後の子孫フォルダを返す（子がなければ path 自身）
fn last_descendant_dir(path: &std::path::Path) -> PathBuf {
    let children = sorted_subdirs(path);
    match children.last() {
        Some(last) => last_descendant_dir(last),
        None => path.to_path_buf(),
    }
}

/// path 以下のすべてのサブフォルダ（path 自身を含む）を再帰的に収集する。
fn walk_dirs_recursive(path: &Path, out: &mut Vec<PathBuf>, cancel: &AtomicBool) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    if !path.is_dir() {
        return;
    }
    out.push(path.to_path_buf());
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk_dirs_recursive(&p, out, cancel);
            }
        }
    }
}

/// 画像1枚をデコード・エンコード・カタログ保存する。成功時は WebP バイト数を返す。
/// load_one_cached と違い、mpsc 送信・ログ出力・進捗更新は行わないバッチ処理専用版。
fn build_and_save_one(
    path: &Path,
    catalog: &crate::catalog::CatalogDb,
    mtime: i64,
    file_size: i64,
    thumb_px: u32,
    thumb_quality: u8,
) -> Option<usize> {
    // 拡張子ベース → マジックバイト fallback（load_one_cached と同じ方針）
    let img = image::open(path)
        .or_else(|_| {
            use std::io::BufReader;
            let f = std::fs::File::open(path)?;
            image::ImageReader::new(BufReader::new(f))
                .with_guessed_format()
                .map_err(image::ImageError::IoError)?
                .decode()
        })
        .ok()?;

    let (webp_data, w, h) =
        crate::catalog::encode_thumb_webp(&img, thumb_px, thumb_quality as f32)?;
    let name = path.file_name()?.to_str()?;
    catalog.save(name, mtime, file_size, w, h, &webp_data).ok()?;
    Some(webp_data.len())
}

/// バイト数を MB / GB 単位の文字列にフォーマットする（cache_manager と同じ方式）。
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// 小さいバイト数 (サムネイル単体) を KB / MB の文字列にフォーマット。
fn format_bytes_small(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

/// 整数を 3 桁区切りにフォーマット (例: 1234 → "1,234")
fn format_count(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

/// 統計ダイアログのヒストグラムを ASCII バー + 件数で描画する。
/// `label_fn` がバケットインデックスから左端ラベルを返す。
fn draw_histogram(
    ui: &mut egui::Ui,
    hist: &[u64],
    label_fn: impl Fn(usize) -> String,
) {
    const MAX_BAR_WIDTH: usize = 32;
    let max_count = hist.iter().copied().max().unwrap_or(0);
    if max_count == 0 {
        ui.label("  (データなし)");
        return;
    }

    // モノスペースフォントで整列
    let font = egui::FontId::monospace(12.0);
    for (bucket, &count) in hist.iter().enumerate() {
        // 末尾の 0 連続をトリミングしない (分布の全体像が見えるように)
        let label = label_fn(bucket);
        let bar_len = ((count as f64 / max_count as f64) * MAX_BAR_WIDTH as f64) as usize;
        let bar: String = std::iter::repeat('=').take(bar_len).collect();
        let count_str = format_count(count);
        let line = format!(
            "  {label}  {bar:<MAX_BAR_WIDTH$}  {count_str:>8}",
            MAX_BAR_WIDTH = MAX_BAR_WIDTH,
        );
        ui.label(egui::RichText::new(line).font(font.clone()));
    }
}

/// 統計ダイアログのフォーマット別件数を ASCII バー + 件数で描画する。
fn draw_format_rows(ui: &mut egui::Ui, rows: &[(&str, u64)]) {
    const MAX_BAR_WIDTH: usize = 32;
    let max_count = rows.iter().map(|(_, c)| *c).max().unwrap_or(0);
    if max_count == 0 {
        ui.label("  (データなし)");
        return;
    }
    let font = egui::FontId::monospace(12.0);
    for (label, count) in rows {
        let bar_len = ((*count as f64 / max_count as f64) * MAX_BAR_WIDTH as f64) as usize;
        let bar: String = std::iter::repeat('=').take(bar_len).collect();
        let count_str = format_count(*count);
        let line = format!(
            "  {label}  {bar:<MAX_BAR_WIDTH$}  {count_str:>8}",
            MAX_BAR_WIDTH = MAX_BAR_WIDTH,
        );
        ui.label(egui::RichText::new(line).font(font.clone()));
    }
}

/// サムネイル画質プレビュー用: 実グリッドと同じ `cell_w × cell_h` のセルを描画する。
/// 白背景 + 4px パディング、画像はアスペクト保持で中央配置（draw_cell と同じ方式）。
/// クリック可能で、クリック時は Response.clicked() が true になる。
fn tq_draw_preview(
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

/// パス直下のサブフォルダを名前順で返す（隠しフォルダは含む）
fn sorted_subdirs(path: &std::path::Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    dirs.sort_by(|a, b| {
        a.to_string_lossy()
            .to_lowercase()
            .cmp(&b.to_string_lossy().to_lowercase())
    });
    dirs
}

/// Windows のファイルシステムは大文字小文字を区別しないため小文字化して比較
fn path_eq(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// items の中で current から delta 分（±1）移動した画像の item index を返す。
/// 境界では None を返す（ラップアラウンドなし）。
#[allow(dead_code)]
fn adjacent_image_idx(items: &[GridItem], current: usize, delta: i32) -> Option<usize> {
    let image_indices: Vec<usize> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| matches!(item, GridItem::Image(_)).then_some(i))
        .collect();
    let pos     = image_indices.iter().position(|&i| i == current)?;
    let new_pos = (pos as i32 + delta).clamp(0, image_indices.len() as i32 - 1) as usize;
    if new_pos == pos { None } else { Some(image_indices[new_pos]) }
}

/// items の中で current から delta 分（±1）移動した「表示可能」アイテム（画像＋動画）の
/// item index を返す。境界では None を返す（ラップアラウンドなし）。
fn adjacent_navigable_idx(items: &[GridItem], current: usize, delta: i32) -> Option<usize> {
    let nav_indices: Vec<usize> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            matches!(item, GridItem::Image(_) | GridItem::Video(_)).then_some(i)
        })
        .collect();
    let pos     = nav_indices.iter().position(|&i| i == current)?;
    let new_pos = (pos as i32 + delta).clamp(0, nav_indices.len() as i32 - 1) as usize;
    if new_pos == pos { None } else { Some(nav_indices[new_pos]) }
}

/// パスに関連付けられたデフォルトアプリケーション（外部プレイヤー）で開く。
fn open_external_player(path: &Path) {
    let path_str = path.to_string_lossy().into_owned();
    crate::logger::log(format!("open_external_player: {path_str}"));
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", &path_str])
        .spawn();
}
