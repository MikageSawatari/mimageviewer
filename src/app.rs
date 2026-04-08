use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
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
    Pending,
    Loaded(egui::TextureHandle),
    #[allow(dead_code)]
    Failed,
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
    tx: mpsc::Sender<(usize, Option<egui::ColorImage>)>,
    rx: mpsc::Receiver<(usize, Option<egui::ColorImage>)>,
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

    // ── キャッシュ管理ポップアップ ───────────────────────────────
    show_cache_manager: bool,
    /// キャッシュ管理の「◯日以上古い」入力値
    cache_manager_days: u32,
    /// 開いたときに取得するキャッシュ統計: (フォルダ数, 合計バイト)
    cache_manager_stats: Option<(usize, u64)>,
    /// 削除後の結果メッセージ
    cache_manager_result: Option<String>,

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
            fullscreen_idx: None,
            fs_cache: std::collections::HashMap::new(),
            fs_pending: std::collections::HashMap::new(),
            show_favorites_editor: false,
            show_preferences: false,
            pref_manual_threads: 4,
            show_cache_manager: false,
            cache_manager_days: 90,
            cache_manager_stats: None,
            cache_manager_result: None,
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
        all_media.sort_by(|(a, _, _, _), (b, _, _, _)| {
            let a_name = a.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
            let b_name = b.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
            a_name.cmp(&b_name)
        });

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

        // 動画サムネイル用リスト: (item_idx, path)
        let video_items: Vec<(usize, PathBuf)> = all_media.iter()
            .enumerate()
            .filter_map(|(i, (p, is_video, _, _))| {
                if *is_video { Some((folder_count + i, p.clone())) } else { None }
            })
            .collect();

        // 画像リスト（カタログ処理用）: (item_idx, path, mtime, file_size)
        // item_idx は all_media 内の位置 + folder_count（動画と混在）
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

        let cache_map = if let Some(ref cat) = catalog_arc {
            cat.load_all().unwrap_or_default()
        } else {
            std::collections::HashMap::new()
        };
        crate::logger::log(format!("  catalog: {} entries in DB", cache_map.len()));

        // ── キャッシュ済み / 要デコードに分類 ────────────────────────
        // cached:      (item_idx, jpeg_data)
        // needs_decode: (item_idx, path, mtime, file_size)
        let mut cached: Vec<(usize, Vec<u8>)> = Vec::new();
        let mut needs_decode: Vec<(usize, PathBuf, i64, i64)> = Vec::new();

        for (item_idx, img_path, mtime, file_size) in images.iter() {
            let filename = img_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if let Some(entry) = cache_map.get(filename) {
                if entry.mtime == *mtime && entry.file_size == *file_size {
                    cached.push((*item_idx, entry.jpeg_data.clone()));
                    continue;
                }
            }
            needs_decode.push((*item_idx, img_path.clone(), *mtime, *file_size));
        }
        crate::logger::log(format!(
            "  catalog: {} cached hits, {} need decode",
            cached.len(),
            needs_decode.len()
        ));

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

        // ── 可視範囲優先に4分割 ───────────────────────────────────────
        let cols = self.settings.grid_cols.max(1);
        let cell_h = self.last_cell_h.max(1.0);
        let first_vis_item = (self.scroll_offset_y / cell_h) as usize * cols;
        let vis_rows = (self.last_viewport_h / cell_h).ceil() as usize + 1;
        let last_vis_item = first_vis_item + vis_rows * cols;

        let is_visible = |i: usize| i >= first_vis_item && i < last_vis_item;

        let (vis_cached, rest_cached): (Vec<_>, Vec<_>) =
            cached.into_iter().partition(|(i, _)| is_visible(*i));
        let (vis_needs, rest_needs): (Vec<_>, Vec<_>) =
            needs_decode.into_iter().partition(|(i, _, _, _)| is_visible(*i));

        crate::logger::log(format!(
            "  visible: {} cached + {} new  |  rest: {} cached + {} new",
            vis_cached.len(), vis_needs.len(), rest_cached.len(), rest_needs.len()
        ));

        // ── 進捗カウンタをリセット ────────────────────────────────────
        // needs_decode の合計枚数を記録し、完了するたびにインクリメントする
        self.cache_gen_total = vis_needs.len() + rest_needs.len();
        self.cache_gen_done = Arc::new(AtomicUsize::new(0));
        let cache_gen_done = Arc::clone(&self.cache_gen_done);

        // フォルダごとに新規プールを作成する（旧フォルダのタスクと競合しない）
        let thumb_threads = self.settings.parallelism.thread_count();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(thumb_threads)
            .build()
            .expect("スレッドプール作成失敗");
        crate::logger::log(format!("  new thread pool created ({thumb_threads} threads)"));

        // Phase 2b 用: スクロール位置に応じて動的優先度を付けるキュー
        // UIスレッドが scroll_hint を毎フレーム更新し、ワーカーが最近傍アイテムを選ぶ
        let scroll_hint = Arc::clone(&self.scroll_hint);
        let rest_queue: Arc<Mutex<Vec<(usize, PathBuf, i64, i64)>>> =
            Arc::new(Mutex::new(rest_needs));

        // 動画サムネイルスレッド用に先にクローンしておく（画像スレッドが move する前に）
        let tx_for_video = tx.clone();
        let cancel_for_video = Arc::clone(&cancel);

        std::thread::spawn(move || {
            pool.install(|| {
                use rayon::prelude::*;

                // Phase 1a: 可視 × キャッシュ済み（JPEG デコードのみ、最速）
                let t = std::time::Instant::now();
                crate::logger::log(format!("  [1a vis-cached ] START {}", vis_cached.len()));
                vis_cached.par_iter().for_each(|(i, jpeg_data)| {
                    if cancel.load(Ordering::Relaxed) { return; }
                    let ci = crate::catalog::jpeg_to_color_image(jpeg_data);
                    let _ = tx.send((*i, ci));
                });
                crate::logger::log(format!("  [1a vis-cached ] END {:.0}ms", t.elapsed().as_secs_f64() * 1000.0));

                // Phase 1b: 可視 × 未キャッシュ（ファイルデコード）
                let t = std::time::Instant::now();
                crate::logger::log(format!("  [1b vis-new    ] START {}", vis_needs.len()));
                vis_needs.par_iter().for_each(|(i, path, mtime, file_size)| {
                    if cancel.load(Ordering::Relaxed) { return; }
                    load_one_cached(path, *i, &tx, catalog_arc.as_deref(), *mtime, *file_size, &cache_gen_done);
                });
                crate::logger::log(format!("  [1b vis-new    ] END {:.0}ms", t.elapsed().as_secs_f64() * 1000.0));

                // Phase 2a: 残り × キャッシュ済み（JPEG デコードのみ、高速なので全件 par_iter）
                let t = std::time::Instant::now();
                crate::logger::log(format!("  [2a rest-cached] START {}", rest_cached.len()));
                rest_cached.par_iter().for_each(|(i, jpeg_data)| {
                    if cancel.load(Ordering::Relaxed) { return; }
                    let ci = crate::catalog::jpeg_to_color_image(jpeg_data);
                    let _ = tx.send((*i, ci));
                });
                crate::logger::log(format!("  [2a rest-cached] END {:.0}ms", t.elapsed().as_secs_f64() * 1000.0));

                // Phase 2b: 残り × 未キャッシュ（動的優先度キュー）
                // ワーカーが scroll_hint に最も近いアイテムを都度選ぶことで
                // ユーザーがスクロールしても現在表示中の行を優先してデコードする
                {
                    let total = rest_queue.lock().unwrap().len();
                    let t = std::time::Instant::now();
                    crate::logger::log(format!("  [2b rest-new   ] START {total}"));
                    rayon::scope(|s| {
                        for _ in 0..thumb_threads {
                            let queue = Arc::clone(&rest_queue);
                            let tx2 = tx.clone();
                            let cancel2 = Arc::clone(&cancel);
                            let hint2 = Arc::clone(&scroll_hint);
                            let cat2 = catalog_arc.clone();
                            let done2 = Arc::clone(&cache_gen_done);
                            s.spawn(move |_| {
                                loop {
                                    if cancel2.load(Ordering::Relaxed) { break; }
                                    let item = {
                                        let mut q = queue.lock().unwrap();
                                        if q.is_empty() { break; }
                                        let vis = hint2.load(Ordering::Relaxed);
                                        // 現在の可視先頭に最も近いインデックスを選択
                                        let best = q.iter().enumerate()
                                            .min_by_key(|(_, (i, _, _, _))| {
                                                let i = *i;
                                                if i < vis { vis - i } else { i - vis }
                                            })
                                            .map(|(idx, _)| idx)
                                            .unwrap(); // q が空でないことは確認済み
                                        q.swap_remove(best)
                                    };
                                    if cancel2.load(Ordering::Relaxed) { break; }
                                    load_one_cached(&item.1, item.0, &tx2, cat2.as_deref(), item.2, item.3, &done2);
                                }
                            });
                        }
                    });
                    crate::logger::log(format!("  [2b rest-new   ] END {:.0}ms", t.elapsed().as_secs_f64() * 1000.0));
                }
            });
        });

        // ── 動画サムネイルを別スレッドで取得（Windows Shell API）─────────
        if !video_items.is_empty() {
            let tx_v = tx_for_video;
            let cancel_v = cancel_for_video;
            let thumb_size = self.last_cell_size.max(256.0) as i32;
            std::thread::spawn(move || {
                for (idx, path) in video_items {
                    if cancel_v.load(Ordering::Relaxed) { break; }
                    let ci = crate::video_thumb::get_video_thumbnail(&path, thumb_size);
                    crate::logger::log(format!(
                        "  video thumb: idx={idx} {}",
                        if ci.is_some() { "ok" } else { "FAIL" }
                    ));
                    let _ = tx_v.send((idx, ci));
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
        while let Ok((i, color_image_opt)) = self.rx.try_recv() {
            if i < self.thumbnails.len() {
                match color_image_opt {
                    Some(color_image) => {
                        let handle = ctx.load_texture(
                            format!("thumb_{i}"),
                            color_image,
                            egui::TextureOptions::LINEAR,
                        );
                        self.thumbnails[i] = ThumbnailState::Loaded(handle);
                    }
                    None => {
                        self.thumbnails[i] = ThumbnailState::Failed;
                    }
                }
                count += 1;
            }
        }
        if count > 0 {
            // 最初の1枚受信時はメインスレッド側のタイムスタンプを記録
            crate::logger::log(format!("  [main] poll_thumbnails: received {count} thumbnail(s)"));
            ctx.request_repaint();
        }
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
        self.poll_prefetch(ctx);

        // ── タイトルバーにキャッシュ生成進捗を表示 ────────────────────
        // cache_gen_total > 0 のときだけ進捗を表示する。
        // 全枚完了したらデフォルトタイトルに戻す。
        {
            let total = self.cache_gen_total;
            let done = self.cache_gen_done.load(Ordering::Relaxed);
            let title = if total > 0 && done < total {
                format!("mimageviewer - キャッシュ生成中 ({}/{})", done, total)
            } else {
                "mimageviewer".to_string()
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }

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
                Some(ThumbnailState::Loaded(h)) => Some(h.clone()),
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
                    ui.separator();
                    if ui.button("キャッシュ管理").clicked() {
                        let cache_dir = crate::catalog::default_cache_dir();
                        self.cache_manager_stats = Some(crate::catalog::cache_stats(&cache_dir));
                        self.cache_manager_result = None;
                        self.show_cache_manager = true;
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

        // ── お気に入り編集ポップアップ ───────────────────────────────
        if self.show_favorites_editor {
            let mut open = true;
            let mut swap: Option<(usize, usize)> = None;
            let mut remove: Option<usize> = None;

            egui::Window::new("お気に入りの編集")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
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

            egui::Window::new("キャッシュ管理")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
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

        // ── 環境設定ポップアップ ─────────────────────────────────────
        if self.show_preferences {
            let mut open = true;
            let mut apply = false;
            let mut cancel = false;

            egui::Window::new("環境設定")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.set_min_width(300.0);

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

                    ui.heading("先読み設定");
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

        // ── ツールバー（列数・アスペクト比の即時切り替え）────────────
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
            ThumbnailState::Loaded(tex) => {
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
            ThumbnailState::Pending => {
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
                ThumbnailState::Loaded(tex) => {
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
                ThumbnailState::Pending => {
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

/// 1枚の画像をデコードしてサムネイルを生成し、カタログに保存してチャネルへ送信する。
/// catalog が None の場合はカタログへの保存をスキップする。
/// gen_done は処理完了時にインクリメントする進捗カウンタ。
fn load_one_cached(
    path: &Path,
    idx: usize,
    tx: &mpsc::Sender<(usize, Option<egui::ColorImage>)>,
    catalog: Option<&crate::catalog::CatalogDb>,
    mtime: i64,
    file_size: i64,
    gen_done: &Arc<AtomicUsize>,
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
            .map_err(|e| image::ImageError::IoError(e))?
            .decode()
    });

    match img_result {
        Ok(img) => {
            let decode_ms = t.elapsed().as_secs_f64() * 1000.0;
            let t2 = std::time::Instant::now();

            match crate::catalog::encode_thumb_jpeg(&img) {
                Some((jpeg_data, w, h)) => {
                    let encode_ms = t2.elapsed().as_secs_f64() * 1000.0;

                    // カタログに保存
                    if let Some(cat) = catalog {
                        if let Err(e) = cat.save(name, mtime, file_size, w, h, &jpeg_data) {
                            crate::logger::log(format!("    idx={idx:>4} catalog save: {e}"));
                        }
                    }

                    // JPEG → ColorImage でチャネルへ送信
                    let color_image = crate::catalog::jpeg_to_color_image(&jpeg_data);
                    let _ = tx.send((idx, color_image));

                    crate::logger::log(format!(
                        "    idx={idx:>4} decode={decode_ms:>6.1}ms encode={encode_ms:>5.1}ms  {name}"
                    ));
                }
                None => {
                    crate::logger::log(format!("    idx={idx:>4} JPEG encode FAIL  {name}"));
                    let _ = tx.send((idx, None));
                }
            }
        }
        Err(e) => {
            crate::logger::log(format!("    idx={idx:>4} FAIL {e}  {name}"));
            let _ = tx.send((idx, None));
        }
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
