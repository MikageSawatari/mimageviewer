use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};

use eframe::egui;

use crate::folder_tree::{
    navigate_folder_with_skip, next_folder_dfs, prev_folder_dfs, walk_dirs_recursive,
    SUPPORTED_EXTENSIONS, SUPPORTED_VIDEO_EXTENSIONS,
};
use crate::fs_animation::{decode_apng_frames, decode_gif_frames, FsCacheEntry, FsLoadResult};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::thumb_loader::{
    build_and_save_one, compute_display_px, process_load_request, CacheDecision, LoadRequest,
    ThumbMsg,
};
use crate::ui_helpers::{
    adjacent_navigable_idx, draw_play_icon, natural_sort_key, open_external_player, truncate_name,
};

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

    // ── 環境設定ポップアップ ─────────────────────────────────────
    pub(crate) show_preferences: bool,
    /// 環境設定ダイアログ内の一時的な並列度編集値（Manual時の数値）
    pub(crate) pref_manual_threads: usize,

    // ── キャッシュ生成設定ポップアップ (段階 C) ──────────────────
    pub(crate) show_cache_policy_dialog: bool,

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
    pub(crate) show_thumb_quality_dialog: bool,
    /// サンプル画像 (デコード済み、ダイアログを閉じるまで保持)
    pub(crate) tq_sample: Option<image::DynamicImage>,
    /// サンプル画像のパス表示用
    pub(crate) tq_sample_path: Option<PathBuf>,
    /// サンプル画像の元ファイルサイズ (bytes)
    pub(crate) tq_sample_original_size: u64,
    /// パネル A: サイズ (long side px)
    pub(crate) tq_a_size: u32,
    /// パネル A: 品質 (1–100)
    pub(crate) tq_a_quality: u8,
    /// パネル A: プレビューテクスチャ
    pub(crate) tq_a_texture: Option<egui::TextureHandle>,
    /// パネル A: エンコード後のバイト数
    pub(crate) tq_a_bytes: usize,
    /// パネル B: サイズ
    pub(crate) tq_b_size: u32,
    /// パネル B: 品質
    pub(crate) tq_b_quality: u8,
    /// パネル B: プレビューテクスチャ
    pub(crate) tq_b_texture: Option<egui::TextureHandle>,
    /// パネル B: エンコード後のバイト数
    pub(crate) tq_b_bytes: usize,
    /// true = A/B 比較の全画面オーバーレイ表示中
    pub(crate) tq_fullscreen: bool,
    /// 全画面 A/B 比較時の縦線位置（0.0=すべて B、1.0=すべて A、中央は 0.5）
    pub(crate) tq_fs_divider: f32,

    // ── キャッシュ作成ポップアップ ───────────────────────────────
    pub(crate) show_cache_creator: bool,
    /// 各お気に入りのチェック状態（settings.favorites と同じ長さ）
    pub(crate) cache_creator_checked: Vec<bool>,
    /// 実行中フラグ（UI ボタンの有効/無効とポーリング制御）
    pub(crate) cache_creator_running: bool,
    /// カウントフェーズ中フラグ（total 未確定）
    pub(crate) cache_creator_counting: Arc<AtomicBool>,
    /// 対象フォルダ総数（Pass 1 完了後に確定）
    pub(crate) cache_creator_total: Arc<AtomicUsize>,
    /// 処理済みフォルダ数
    pub(crate) cache_creator_done: Arc<AtomicUsize>,
    /// キャッシュ容量 (バイト単位、累積加算)
    pub(crate) cache_creator_cache_size: Arc<AtomicU64>,
    /// キャンセルトークン
    pub(crate) cache_creator_cancel: Arc<AtomicBool>,
    /// 現在処理中のフォルダパス表示用
    pub(crate) cache_creator_current: Arc<Mutex<String>>,
    /// 完了シグナル（表示切替用）
    pub(crate) cache_creator_finished: Arc<AtomicBool>,
    /// 完了後のメッセージ
    pub(crate) cache_creator_result: Option<String>,

    // ── アドレスバーフォーカス管理 ───────────────────────────────
    /// true のときアドレスバーが入力中 → キーショートカットを無効化
    pub(crate) address_has_focus: bool,

    // ── フォルダ履歴（スクロール位置・選択状態の復元用）────────────
    /// フォルダパス → (scroll_offset_y, selected_idx)
    pub(crate) folder_history: std::collections::HashMap<PathBuf, (f32, Option<usize>)>,

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
        for (_, list) in groups.iter_mut() {
            match self.settings.sort_order {
                crate::settings::SortOrder::FileName => {
                    list.sort_by(|a, b| {
                        let an = crate::zip_loader::entry_basename(&a.entry_name).to_lowercase();
                        let bn = crate::zip_loader::entry_basename(&b.entry_name).to_lowercase();
                        an.cmp(&bn)
                    });
                }
                crate::settings::SortOrder::Numeric => {
                    list.sort_by(|a, b| {
                        let an = crate::zip_loader::entry_basename(&a.entry_name);
                        let bn = crate::zip_loader::entry_basename(&b.entry_name);
                        natural_sort_key(an).cmp(&natural_sort_key(bn))
                    });
                }
                crate::settings::SortOrder::DateAsc => {
                    list.sort_by(|a, b| a.mtime.cmp(&b.mtime));
                }
                crate::settings::SortOrder::DateDesc => {
                    list.sort_by(|a, b| b.mtime.cmp(&a.mtime));
                }
            }
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
            self.spawn_video_thread(tx, cancel, video_items);
        }

        // ── 履歴復元 + last_folder 保存 ──
        if let Some(&(scroll, sel)) = self.folder_history.get(&source_path) {
            self.scroll_offset_y = scroll;
            self.selected = sel;
            if sel.is_some() {
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
    ) {
        let thumb_size = self.last_cell_size.max(256.0) as i32;
        let stats = Arc::clone(&self.stats);

        std::thread::spawn(move || {
            for (idx, path, file_size) in video_items {
                if cancel.load(Ordering::Relaxed) { break; }
                let ci = crate::video_thumb::get_video_thumbnail(&path, thumb_size);
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
                let _ = tx.send((idx, ci, false));
            }
        });
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
            // 画像 / ZIP 内画像のみ要求。フォルダ・動画・セパレータはスキップ
            let Some((mtime, file_size)) = self.image_metas.get(i).copied().flatten() else {
                continue;
            };
            let req = match self.items.get(i) {
                Some(GridItem::Image(p)) => LoadRequest {
                    idx: i, path: p.clone(), mtime, file_size,
                    skip_cache: false, zip_entry: None,
                },
                Some(GridItem::ZipImage { zip_path, entry_name }) => LoadRequest {
                    idx: i, path: zip_path.clone(), mtime, file_size,
                    skip_cache: false, zip_entry: Some(entry_name.clone()),
                },
                _ => continue,
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
            let req = match self.items.get(i) {
                Some(GridItem::Image(p)) => LoadRequest {
                    idx: i, path: p.clone(), mtime, file_size,
                    skip_cache: true, zip_entry: None,
                },
                Some(GridItem::ZipImage { zip_path, entry_name }) => LoadRequest {
                    idx: i, path: zip_path.clone(), mtime, file_size,
                    skip_cache: true, zip_entry: Some(entry_name.clone()),
                },
                _ => continue,
            };
            upgrade_reqs.push(req);
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
            let open_result = if let Some(bytes) = zip_bytes {
                image::load_from_memory(&bytes)
            } else {
                image::open(&path)
            };
            match open_result {
                Ok(img) => {
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
    pub(crate) fn open_thumb_quality_dialog(&mut self, ctx: &egui::Context) {
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

    pub(crate) fn reencode_tq_panel(&mut self, ctx: &egui::Context, is_a: bool) {
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

    pub(crate) fn close_thumb_quality_dialog(&mut self) {
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
    pub(crate) fn start_cache_creation(&mut self) {
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
            // タスク 3: ZIP セパレータ (章タイトル表示)
            let separator_text: Option<String> = match self.items.get(fs_idx) {
                Some(GridItem::ZipSeparator { dir_display }) => Some(dir_display.clone()),
                _ => None,
            };
            let is_separator = separator_text.is_some();
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
            // 画像のみ「高解像度読込中」表示が必要（動画・セパレータは不要）
            let is_loading = !is_video && !is_separator && !self.fs_cache.contains_key(&fs_idx);

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
                                // 画像 / セパレータ: 左半分 → 前、右半分 → 次
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

                            // ── 画像 / 動画 / セパレータ表示 ──────────
                            if let Some(sep) = separator_text.as_ref() {
                                // タスク 3: ZIP セパレータ → 章タイトル画面
                                // レイアウト (サムネイルセルと同じ方針):
                                //   - 中央: フォルダ名 (大きく)
                                //   - 下部: "── 作品の区切り ──" 案内
                                let title_size = (full_rect.height() * 0.12).clamp(48.0, 120.0);
                                let sub_size = (full_rect.height() * 0.030).clamp(20.0, 36.0);

                                // 控えめな背景ハイライト (フォルダ名の周囲のみ)
                                ui.painter().rect_filled(
                                    egui::Rect::from_center_size(
                                        full_rect.center(),
                                        egui::vec2(
                                            full_rect.width() * 0.85,
                                            title_size * 2.2,
                                        ),
                                    ),
                                    16.0,
                                    egui::Color32::from_rgba_unmultiplied(30, 45, 80, 180),
                                );
                                // フォルダ名 (中央、大きく)
                                ui.painter().text(
                                    full_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    sep,
                                    egui::FontId::proportional(title_size),
                                    egui::Color32::WHITE,
                                );
                                // 「作品の区切り」案内 (画面下部)
                                ui.painter().text(
                                    egui::pos2(
                                        full_rect.center().x,
                                        full_rect.max.y - 48.0,
                                    ),
                                    egui::Align2::CENTER_BOTTOM,
                                    "── 作品の区切り ──",
                                    egui::FontId::proportional(sub_size),
                                    egui::Color32::from_rgb(150, 180, 220),
                                );
                            } else {
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
                            // セパレータでない + 何らかのテクスチャが表示されている場合のみ表示
                            let has_any_tex = tex.is_some() || thumb_tex.is_some();
                            if is_loading && has_any_tex {
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

        self.show_favorites_editor_dialog(ctx);
        self.show_cache_manager_dialog(ctx);
        self.show_cache_creator_dialog(ctx);
        self.show_thumb_quality_dialog_window(ctx);
        self.show_thumb_quality_fullscreen_overlay(ctx);
        self.show_preferences_dialog(ctx);
        self.show_cache_policy_dialog(ctx);
        self.show_stats_dialog_window(ctx);
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
                                        Some(GridItem::Image(_))
                                        | Some(GridItem::ZipImage { .. })
                                        | Some(GridItem::ZipSeparator { .. }) => {
                                            self.open_fullscreen(idx)
                                        }
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
        GridItem::ZipImage { entry_name: _, .. } => match thumb {
            ThumbnailState::Loaded { tex, .. } => {
                let tex_size = tex.size_vec2();
                let scale = (inner.width() / tex_size.x).min(inner.height() / tex_size.y);
                let img_rect =
                    egui::Rect::from_center_size(inner.center(), tex_size * scale);
                painter.image(
                    tex.id(),
                    img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
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
        },
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
