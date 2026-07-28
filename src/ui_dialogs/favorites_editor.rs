//! 「お気に入り」ダイアログ (v0.8.0 で旧 `favorites_editor` + `index_manager` を統合)。
//!
//! 1 つのダイアログで:
//! - 表示名 / 並べ替え / 削除
//! - 名前索引 / メタ索引の ON/OFF
//! - メタ索引の初期スキャン状態 (✅ 完了 / ⏳ スキャン中)
//! - サムネイル一括作成ダイアログの起動
//!
//! を扱う。旧 2 つのダイアログを別にしていた頃は「フラグは A で切り替え、状態は B で
//! 見る」という導線で分かりにくかったため 1 本化した。
//!
//! 索引メンテナンスは notify-rs + 起動時スキャンで自動同期される前提。
//! 手動再構築・一括作成系の UI は v0.8.0 で外し、索引が壊れたときはプロセス再起動で
//! 初期スキャンから作り直す運用にしている。

use std::time::Duration;

use eframe::egui;

use crate::app::App;
use crate::indexer_supervisor::SupervisorStats;
use crate::ui_helpers::{format_bytes, format_count, truncate_name};

/// 全文検索インデックスの on-disk サイズ (bytes) を集計する。
///
/// - `name_index_db`: お気に入り単位の名前索引 (SQLite)
/// - `fts_meta_db`: 管理メタ専用 DB (SQLite、status / mtime / size のみ)
/// - `fts_index_dir`: Tantivy インデックス (segment ファイル群の合計、原文 STORED 含む)
///
/// すべて `stat()` ベースなので数ミリ秒で完了する。ダイアログ表示中はファイル件数と
/// 同じ 1.5s 間隔で更新される (ingest で増えるので)。存在しないファイル /
/// ディレクトリは 0 扱い。
#[derive(Copy, Clone, Default)]
pub(crate) struct IndexDiskSizes {
    pub(crate) name_index_db: u64,
    pub(crate) fts_meta_db: u64,
    pub(crate) fts_index_dir: u64,
}

/// お気に入りごとの索引件数 (名前索引 / メタ索引) のスナップショット。
///
/// ダイアログ open 時に 1 回だけ計算してキャッシュする。毎フレーム数十回の
/// `COUNT(*)` を走らせないため。close 時に破棄。
///
/// - `name_counts[fav_id]`: search_index.db の当 favorite_root 配下のエントリ数
/// - `meta_counts[fav_id]`: fts_meta.db の当 favorite_id × status=Ok の件数
/// - `name_total` / `meta_total`: 上記の合計 (索引サイズ表示用の総件数)
#[derive(Default, Clone)]
pub(crate) struct IndexFileCounts {
    pub(crate) name_counts: std::collections::HashMap<uuid::Uuid, u64>,
    pub(crate) meta_counts: std::collections::HashMap<uuid::Uuid, u64>,
    pub(crate) name_total: u64,
    pub(crate) meta_total: u64,
}

/// 全 favorite 分の件数を集計する。
///
/// **DB lock 1 回ずつで済む形** で取得する (`GROUP BY` 一括クエリ)。worker thread から
/// 呼ばれることを想定しており、UI スレッドで実行すると ingest と競合する。
///
/// クエリ自体は `LIKE` 等を使わず PK / 部分インデックスを単純スキャンするので、
/// 657k 行クラスでも数十 ms オーダーで完了する。
pub(crate) fn collect_counts(
    name_db: Option<&crate::search_index_db::SearchIndexDb>,
    meta_db: Option<&crate::fts_meta::FtsMetaDb>,
    favorites: &[(uuid::Uuid, std::path::PathBuf)],
) -> IndexFileCounts {
    let mut out = IndexFileCounts::default();
    let name_map = name_db
        .and_then(|db| db.count_grouped_by_favorite_root().ok())
        .unwrap_or_default();
    let meta_map = meta_db
        .and_then(|db| db.count_ok_grouped_by_favorite().ok())
        .unwrap_or_default();
    for (fav_id, fav_path) in favorites {
        let key = crate::search_index_db::normalize_path(fav_path);
        let n = name_map.get(&key).copied().unwrap_or(0);
        let m = meta_map.get(fav_id).copied().unwrap_or(0);
        out.name_counts.insert(*fav_id, n);
        out.meta_counts.insert(*fav_id, m);
        out.name_total += n;
        out.meta_total += m;
    }
    out
}

pub(crate) fn compute_index_disk_sizes() -> IndexDiskSizes {
    let data_dir = crate::data_dir::get();
    let file_size =
        |p: &std::path::Path| -> u64 { std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) };
    let dir_size = |p: &std::path::Path| -> u64 {
        let Ok(entries) = std::fs::read_dir(p) else {
            return 0;
        };
        entries
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum()
    };
    // SQLite は WAL / SHM も DB 本体のサイズに含める (実際のディスク使用量)
    let sqlite_size = |p: &std::path::Path| -> u64 {
        let mut total = file_size(p);
        let wal = p.with_extension(format!(
            "{}-wal",
            p.extension().and_then(|e| e.to_str()).unwrap_or("db")
        ));
        let shm = p.with_extension(format!(
            "{}-shm",
            p.extension().and_then(|e| e.to_str()).unwrap_or("db")
        ));
        total += file_size(&wal);
        total += file_size(&shm);
        total
    };
    IndexDiskSizes {
        name_index_db: sqlite_size(&data_dir.join("search_index.db")),
        fts_meta_db: sqlite_size(&data_dir.join("fts_meta.db")),
        fts_index_dir: dir_size(&data_dir.join("fts_index")),
    }
}

impl App {
    pub(crate) fn show_favorites_editor_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_favorites_editor {
            return;
        }
        let mut open = true;
        // 即時反映設計: OK/Cancel を廃止し、編集はその場で反映する。
        // チェックボックス / 表示名 / 並べ替え / 削除 / 一括操作 いずれも変更を
        // 検出したフレームで settings.save() と必要な index 側の副作用を走らせる。
        let mut close_requested = false;
        // このフレームで反映した差分を集めて、ループ後にまとめて apply する
        // (ループ中に self を再帰的に &mut 借りる回避)。
        let mut name_index_toggles: Vec<(uuid::Uuid, std::path::PathBuf, bool)> = Vec::new();
        let mut meta_index_toggles: Vec<(uuid::Uuid, bool)> = Vec::new();
        let mut favorite_default_toggles: Vec<(uuid::Uuid, String, bool)> = Vec::new();
        let mut any_setting_dirty = false;
        let mut swap: Option<(usize, usize)> = None;
        let mut remove: Option<usize> = None;
        let mut open_cache_creator = false;
        let has_favorites = !self.settings.favorites.is_empty();
        let (safe_rect, dialog_size, min_dialog_size) =
            favorites_editor_dialog_geometry(ctx.content_rect(), has_favorites);
        let safe_size = safe_rect.size();
        let dialog_rect = egui::Rect::from_center_size(safe_rect.center(), dialog_size);
        let escape_pressed = self.dialog_escape_pressed(ctx);
        // save 後にトレイ checkmark を同期するかの判定材料。
        let old_pause_minimized = self.settings.pause_indexer_while_minimized;

        // 事前にインデクサ情報を取り出し (borrow 競合回避)
        let startup_diag = self.indexer_manager.as_ref().map(|m| m.startup_diag());
        // インデックスサイズ + 件数キャッシュ。
        //
        // Codex 指摘 (2026-04): UI スレッドで `COUNT(*)` を N 回叩くと、毎回
        // SQLite connection mutex を取って ingest_worker (`upsert_meta_ok` /
        // `delete_paths`) や bulk indexer の writer が短時間待たされる。対策:
        //  1. 個別 COUNT を `GROUP BY favorite_id/favorite_root` の単一クエリに集約
        //     (DB lock 取得回数を 2N → 2 に削減)
        //  2. UI スレッドではなく worker thread で実行 (`favorites_index_refresh_rx`)
        //  3. 間隔を 5s に緩和 (リアルタイム性は不要、バッジで分かれば良い)
        const REFRESH: std::time::Duration = std::time::Duration::from_secs(5);
        // 直前の worker 結果が来ていれば回収する。
        if let Some(rx) = self.favorites_index_refresh_rx.as_ref() {
            match rx.try_recv() {
                Ok((counts, sizes)) => {
                    self.favorites_index_count_cache = Some((std::time::Instant::now(), counts));
                    self.favorites_index_size_cache = Some(sizes);
                    self.favorites_index_refresh_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.favorites_index_refresh_rx = None;
                }
            }
        }
        // 必要なら worker をスポーン (in-flight 中は重複起動しない)。
        let counts_stale = self
            .favorites_index_count_cache
            .as_ref()
            .map(|(t, _)| t.elapsed() >= REFRESH)
            .unwrap_or(true);
        if counts_stale && self.favorites_index_refresh_rx.is_none() {
            let name_db = self.search_index_db.as_ref().cloned();
            let meta_db = self.indexer_manager.as_ref().map(|m| m.clone_fts_meta());
            let fav_ids: Vec<(uuid::Uuid, std::path::PathBuf)> = self
                .settings
                .favorites
                .iter()
                .map(|f| (f.id, f.path.clone()))
                .collect();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("fav-index-counts".into())
                .spawn(move || {
                    let counts = collect_counts(name_db.as_deref(), meta_db.as_deref(), &fav_ids);
                    let sizes = compute_index_disk_sizes();
                    let _ = tx.send((counts, sizes));
                })
                .ok();
            self.favorites_index_refresh_rx = Some(rx);
        }
        // 初回はキャッシュがまだ無いので、ブロックせずに空表示で進める
        // (worker が完了したら次フレームで反映される)。
        // ↓ 以降は `egui::Window::show` クロージャで `self.settings.favorites` を
        // 借用するので、キャッシュからは Copy 値だけ抜いて閉じこもらない形にする。
        let (name_total, meta_total) = self
            .favorites_index_count_cache
            .as_ref()
            .map(|(_, c)| (c.name_total, c.meta_total))
            .unwrap_or((0, 0));
        let row_counts: Vec<(u64, u64)> = {
            let cache = self.favorites_index_count_cache.as_ref().map(|(_, c)| c);
            self.settings
                .favorites
                .iter()
                .map(|fav| {
                    let n = cache
                        .and_then(|c| c.name_counts.get(&fav.id).copied())
                        .unwrap_or(0);
                    let m = cache
                        .and_then(|c| c.meta_counts.get(&fav.id).copied())
                        .unwrap_or(0);
                    (n, m)
                })
                .collect()
        };
        let reconciling = self
            .indexer_manager
            .as_ref()
            .is_some_and(|m| m.is_reconciling());
        let stats_by_id: std::collections::HashMap<uuid::Uuid, SupervisorStats> = self
            .indexer_manager
            .as_ref()
            .map(|m| m.all_stats())
            .unwrap_or_default()
            .into_iter()
            .map(|v| (v.favorite_id, v.stats))
            .collect();
        // 名前索引側の stats も同様に集める (名前索引 supervisor は App 直下管理)
        let name_stats_by_id: std::collections::HashMap<
            uuid::Uuid,
            crate::name_index_supervisor::NameIndexStats,
        > = self
            .name_index_supervisors
            .iter()
            .map(|(id, h)| (*id, h.snapshot_stats()))
            .collect();

        egui::Window::new("お気に入り")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos(dialog_rect.min)
            .default_size(dialog_size)
            .min_size(min_dialog_size)
            .max_size(safe_size)
            .constrain_to(safe_rect)
            .show(ctx, |ui| {
                // タイトルバー以外の本文は単一の ScrollArea にまとめる。一覧や
                // インデクサへ別の縦スクロールを持たせると、下部操作へ到達しづらく
                // wheel の所有先も分かりにくくなるため、本文全体で 1 本だけにする。
                let body_height = ui.available_height().max(1.0);
                egui::ScrollArea::vertical()
                    .id_salt("favorites_editor_body_scroll")
                    .auto_shrink([false, false])
                    .max_height(body_height)
                    .show(ui, |ui| {
                        ui.set_min_width((safe_size.x - 40.0).clamp(1.0, 744.0));

                // ── 起動時整合性チェック (インデクサが有効なら表示) ──
                if reconciling {
                    ui.label(
                        egui::RichText::new(
                            "📋 インデックス整合性チェック中… (起動時 reconciliation)",
                        )
                        .color(ui.visuals().warn_fg_color)
                        .size(11.0),
                    );
                    ui.add_space(2.0);
                }
                if let Some(diag) = startup_diag {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("🚀 起動時の整合性チェック:")
                                    .strong()
                                    .size(11.0),
                            );
                            ui.label(
                                egui::RichText::new(format!("{} ms", diag.reconciliation_ms))
                                    .monospace()
                                    .size(11.0),
                            )
                            .on_hover_text(
                                "起動直後に走る reconciliation の所要時間。\n\
                                 Tantivy writer 競合を避けるため同期実行。通常 < 100ms。",
                            );
                            ui.separator();
                            ui.label(
                                egui::RichText::new(format!("I/O 並列度: {}", diag.io_permits))
                                    .size(11.0)
                                    .color(ui.visuals().weak_text_color()),
                            )
                            .on_hover_text(
                                "環境設定「インデクサの速度プロファイル」で変更可。\n\
                                 1=省電力 / 2=標準 / 4=高速",
                            );
                        });

                        // インデックスのディスク使用量とファイル件数。直前の refresh 区間で
                        // 計算済み (1.5s 間隔)。close 時に両方破棄。
                        let sizes = self.favorites_index_size_cache.unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("💾 インデックスサイズ:")
                                    .size(11.0)
                                    .color(ui.visuals().weak_text_color()),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "コンテナ索引 {} ({}件)",
                                    format_bytes(sizes.name_index_db),
                                    format_count(name_total)
                                ))
                                .size(11.0)
                                .color(ui.visuals().weak_text_color()),
                            )
                            .on_hover_text(
                                "コンテナ索引のディスク使用量 (WAL/SHM 込み) と総件数。\n\
                                 お気に入り配下のフォルダ / ZIP / PDF を名前で横断検索 (Ctrl+S)。",
                            );
                            ui.separator();
                            ui.label(
                                egui::RichText::new(format!(
                                    "アイテム索引 {} + {} ({}件)",
                                    format_bytes(sizes.fts_meta_db),
                                    format_bytes(sizes.fts_index_dir),
                                    format_count(meta_total)
                                ))
                                .size(11.0)
                                .color(ui.visuals().weak_text_color()),
                            )
                            .on_hover_text(
                                "アイテム索引のディスク使用量 (WAL/SHM 込み)。\n\
                                 お気に入り配下の画像 / PDF / 動画を、ファイル名・タグ・\n\
                                 EXIF・AI プロンプト等で横断検索 (Ctrl+G)。",
                            );
                        });
                    });
                    ui.add_space(4.0);
                }

                // ── タスクトレイ常駐の導線 (v0.9) ──────────────
                // 起動時スキャンの頁にこの案内を置いて、「スキャンが重い → トレイ常駐で
                // 回避できる」という導線を自然に見せる。環境設定にも同じ項目があるが、
                // 環境設定は項目数が多く発見しにくいため、ここに重複して置くのは意図的。
                ui.group(|ui| {
                    ui.label(
                        egui::RichText::new(
                            "💡 アプリケーションを終了すると、次回起動時に\
                             インデックスの再スキャンが行われます。\
                             終了する代わりにタスクトレイに常駐すると、\
                             起動がスムーズになります。",
                        )
                        .size(11.0)
                        .color(ui.visuals().weak_text_color()),
                    );
                    let tray_before = self.settings.minimize_to_tray_on_close;
                    if ui
                        .checkbox(
                            &mut self.settings.minimize_to_tray_on_close,
                            "アプリを閉じる代わりに、タスクトレイに常駐する \
                             (タスクトレイアイコンから終了できます)",
                        )
                        .changed()
                    {
                        any_setting_dirty = true;
                        // チェックを入れた瞬間に案内ダイアログを出す (毎回表示)。
                        if !tray_before && self.settings.minimize_to_tray_on_close {
                            self.show_tray_enabled_notice = true;
                        }
                    }
                    if self.settings.minimize_to_tray_on_close {
                        ui.add_space(2.0);
                        if ui
                            .checkbox(
                                &mut self.settings.pause_indexer_while_minimized,
                                "常駐中はインデックス更新を一時停止する \
                                 (他アプリの I/O 負荷を抑えたいときに)",
                            )
                            .on_hover_text(
                                "ON にすると、ウィンドウを閉じてトレイに入った後は\n\
                                 初回スキャンも notify-rs 経由の更新も行いません。\n\
                                 ウィンドウを開くと自動で再開し、溜まっていた変更を\n\
                                 順次処理します。OFF でも常駐中は I/O 並列度が\n\
                                 自動で 1 に絞られるので、他アプリへの影響は抑えられます。",
                            )
                            .changed()
                        {
                            any_setting_dirty = true;
                        }
                    }
                });
                ui.add_space(4.0);

                if self.settings.favorites.is_empty() {
                    ui.label("お気に入りはまだ登録されていません。");
                    ui.add_space(4.0);
                } else {
                    ui.label(
                        egui::RichText::new(
                            "お気に入りは以下を索引化して、コンテナ検索 (Ctrl+S) ・\
                             アイテム検索 (Ctrl+G) できます。\
                             チェックを入れた項目はこの場で 1 回全走査し、以降は\
                             ファイルの変更監視と起動時スキャンで自動更新します。",
                        )
                        .weak()
                        .size(11.0),
                    );
                    ui.add_space(6.0);

                    let n = self.settings.favorites.len();
                    egui::Grid::new("fav_edit_grid")
                        .striped(true)
                        .num_columns(7)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                                    // ── ヘッダ (状態は各索引列にインライン) ──
                                    ui.label(egui::RichText::new("番号").strong())
                                        .on_hover_text(
                                            "操作カスタマイズの「お気に入り1を開く」などで使う順番です。",
                                        );
                                    ui.label(egui::RichText::new("表示名").strong());
                                    ui.label(egui::RichText::new("パス").strong());
                                    ui.label(
                                        egui::RichText::new("標準設定を分ける").strong(),
                                    )
                                    .on_hover_text(
                                        "このお気に入り専用の標準設定を持たせます",
                                    );
                                    ui.label(egui::RichText::new("コンテナ索引 (Ctrl+S)").strong())
                                        .on_hover_text(
                                            "フォルダ / ZIP / PDF を名前で横断検索\n\
                                         ✅ = 索引あり / ⏳ = バルク作成中",
                                        );
                                    ui.label(egui::RichText::new("アイテム索引 (Ctrl+G)").strong())
                                        .on_hover_text(
                                            "画像 / PDF / 動画をファイル名・タグ・EXIF・\n\
                                         AI プロンプト等で横断検索\n\
                                         ✅ 監視中 = 初期スキャン完了 + ファイルの変更を追従\n\
                                         ⏳ スキャン中 = アクティブスキャン実行中",
                                        );
                                    ui.label(egui::RichText::new("操作").strong());
                                    ui.end_row();

                                    // 名前索引 supervisor の進捗テキストはダイアログ下部の
                                    // 「バックグラウンドインデクサ」セクションで表示する。

                                    // ── 各行 ──
                                    for i in 0..n {
                                        let fav_id = self.settings.favorites[i].id;
                                        let fav_path = self.settings.favorites[i].path.clone();

                                        ui.label(
                                            egui::RichText::new((i + 1).to_string())
                                                .monospace()
                                                .weak(),
                                        );

                                        // 表示名 (編集可能) — 変更を検出したら即 save
                                        let name_resp = ui.add_sized(
                                            [100.0, 20.0],
                                            egui::TextEdit::singleline(
                                                &mut self.settings.favorites[i].name,
                                            ),
                                        );
                                        if name_resp.changed() {
                                            any_setting_dirty = true;
                                        }

                                        // パス (読み取り専用)
                                        let path_str = fav_path.to_string_lossy().to_string();
                                        ui.label(
                                            egui::RichText::new(truncate_name(&path_str, 40))
                                                .monospace()
                                                .weak(),
                                        )
                                        .on_hover_text(&path_str);

                                        // お気に入り標準: favorite_params 行の有無が ON/OFF。
                                        let favorite_name =
                                            self.settings.favorites[i].name.clone();
                                        let mut separate_standard = self
                                            .adjustment_favorite_params
                                            .contains_key(&fav_id);
                                        let standard_resp =
                                            ui.checkbox(&mut separate_standard, "");
                                        if standard_resp.changed() {
                                            favorite_default_toggles.push((
                                                fav_id,
                                                favorite_name.clone(),
                                                separate_standard,
                                            ));
                                        }
                                        standard_resp.on_hover_text(format!(
                                            "お気に入り「{favorite_name}」に、このお気に入り専用の標準設定を持たせます"
                                        ));

                                        // 名前索引: チェック + 状態インライン
                                        ui.horizontal(|ui| {
                                            let struct_resp = ui.checkbox(
                                                &mut self.settings.favorites[i]
                                                    .auto_index_structure,
                                                "",
                                            );
                                            if struct_resp.changed() {
                                                let new_val =
                                                    self.settings.favorites[i].auto_index_structure;
                                                name_index_toggles.push((
                                                    fav_id,
                                                    fav_path.clone(),
                                                    new_val,
                                                ));
                                                any_setting_dirty = true;
                                            }
                                            let name_on =
                                                self.settings.favorites[i].auto_index_structure;
                                            draw_state_inline(
                                                ui,
                                                name_on,
                                                name_stats_by_id
                                                    .get(&fav_id)
                                                    .map(|s| (s.in_full_scan, s.initial_scan_done)),
                                                row_counts[i].0,
                                            );
                                        });

                                        // メタデータ索引: チェック + 状態インライン
                                        ui.horizontal(|ui| {
                                            let meta_resp = ui.checkbox(
                                                &mut self.settings.favorites[i].auto_index_metadata,
                                                "",
                                            );
                                            if meta_resp.changed() {
                                                let new_val =
                                                    self.settings.favorites[i].auto_index_metadata;
                                                meta_index_toggles.push((fav_id, new_val));
                                                any_setting_dirty = true;
                                            }
                                            let meta_on =
                                                self.settings.favorites[i].auto_index_metadata;
                                            draw_state_inline(
                                                ui,
                                                meta_on,
                                                stats_by_id
                                                    .get(&fav_id)
                                                    .map(|s| (s.in_full_scan, s.initial_scan_done)),
                                                row_counts[i].1,
                                            );
                                        });

                                        // 操作 (↑ ↓ 削除)
                                        ui.horizontal(|ui| {
                                            let up_en = i > 0;
                                            let dn_en = i + 1 < n;
                                            if ui
                                                .add_enabled(up_en, egui::Button::new("↑"))
                                                .clicked()
                                            {
                                                swap = Some((i - 1, i));
                                            }
                                            if ui
                                                .add_enabled(dn_en, egui::Button::new("↓"))
                                                .clicked()
                                            {
                                                swap = Some((i, i + 1));
                                            }
                                            if ui.button("削除").clicked() {
                                                self.favorite_delete_confirm = Some(fav_id);
                                            }
                                        });
                                        ui.end_row();
                                    }
                        });

                    // ── 一括操作 (チェックボックスの一括 ON/OFF のみ) ──
                    // 変化した favorite だけ toggle リストに積んで、ループ後にまとめて
                    // 副作用 (supervisor sync / name bulk 起動 / cleanup) を走らせる。
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("一括操作:").weak());
                        if ui.button("コンテナ 全ON").clicked() {
                            for f in &mut self.settings.favorites {
                                if !f.auto_index_structure {
                                    f.auto_index_structure = true;
                                    name_index_toggles.push((f.id, f.path.clone(), true));
                                    any_setting_dirty = true;
                                }
                            }
                        }
                        if ui.button("コンテナ 全OFF").clicked() {
                            for f in &mut self.settings.favorites {
                                if f.auto_index_structure {
                                    f.auto_index_structure = false;
                                    name_index_toggles.push((f.id, f.path.clone(), false));
                                    any_setting_dirty = true;
                                }
                            }
                        }
                        if ui.button("アイテム 全ON").clicked() {
                            for f in &mut self.settings.favorites {
                                if !f.auto_index_metadata {
                                    f.auto_index_metadata = true;
                                    meta_index_toggles.push((f.id, true));
                                    any_setting_dirty = true;
                                }
                            }
                        }
                        if ui.button("アイテム 全OFF").clicked() {
                            for f in &mut self.settings.favorites {
                                if f.auto_index_metadata {
                                    f.auto_index_metadata = false;
                                    meta_index_toggles.push((f.id, false));
                                    any_setting_dirty = true;
                                }
                            }
                        }
                    });

                    // ── サムネイル一括作成 (I/O が重いため手動バルクのみの位置付け) ──
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button("🖼 サムネを一括作成…")
                            .on_hover_text(
                                "サムネイル一括作成ダイアログを開きます。お気に入り配下\n\
                                 全画像のサムネを先回り生成してキャッシュに保存します\n\
                                 (I/O 量が大きいため手動実行のみ)。",
                            )
                            .clicked()
                        {
                            open_cache_creator = true;
                        }
                    });

                    // ── バックグラウンドインデクサのライブ進捗 ──
                    // メタ索引 supervisor (walker/ingest) と名前索引バルクの両方を
                    // まとめて列挙する。完了済み or アイドルは current_activity が None
                    // なので空欄になる (= 全アイドル)。
                    //
                    // 表示領域は固定高さの ScrollArea にしてある。これは notify-rs が
                    // 別プロセスのファイルコピー等を細かく拾うと、行が一瞬出ては消える
                    // 振動でダイアログ全体の高さが揺れていたため (2026-04 ユーザー指摘)。
                    // 行数によらず常に同じ高さを確保し、内側だけスクロールする。
                    // 個別の (label, message) と、進行中の全 EtaSnapshot を集める。
                    // ETA は集約して全体の残り時間として 1 つだけ表示する (個別 ETA は
                    // パス truncate に巻き込まれて見にくいため、ユーザー指摘で外した)。
                    let mut active: Vec<(String, String)> = Vec::new();
                    let mut all_etas: Vec<crate::indexer_progress::EtaSnapshot> = Vec::new();
                    for fav in &self.settings.favorites {
                        if let Some(s) = stats_by_id.get(&fav.id) {
                            if let Some(msg) = s.current_activity.as_ref() {
                                active.push((format!("{} (メタ)", fav.name), msg.clone()));
                            }
                            if let Some(eta) = s.eta {
                                all_etas.push(eta);
                            }
                        }
                        if let Some(s) = name_stats_by_id.get(&fav.id) {
                            if let Some(msg) = s.current_activity.as_ref() {
                                active.push((format!("{} (名前)", fav.name), msg.clone()));
                            }
                            if let Some(eta) = s.eta {
                                all_etas.push(eta);
                            }
                        }
                    }
                    // 全体 ETA: 並列実行されているので「残り時間 = max(各 remaining_secs)」、
                    // 「処理速度 = Σ(各 rate_per_sec)」。各 supervisor のサンプルがまだ
                    // 揃っていなければ remaining_secs=None なので除外。
                    //
                    // 毎フレーム計算するとレートが 100ms 単位で振動して見づらいので、
                    // **表示文字列を 1 秒間隔でキャッシュ** して値の更新頻度を下げる。
                    const ETA_DISPLAY_INTERVAL: std::time::Duration =
                        std::time::Duration::from_secs(1);
                    let eta_stale = self
                        .favorites_total_eta_cache
                        .as_ref()
                        .map(|(t, _)| t.elapsed() >= ETA_DISPLAY_INTERVAL)
                        .unwrap_or(true);
                    if eta_stale {
                        self.favorites_total_eta_cache =
                            Some((std::time::Instant::now(), aggregate_total_eta(&all_etas)));
                    }
                    let total_eta_text = self
                        .favorites_total_eta_cache
                        .as_ref()
                        .and_then(|(_, s)| s.clone());
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("🔄 バックグラウンドインデクサ")
                                .size(11.0)
                                .color(ui.visuals().warn_fg_color),
                        );
                        if let Some(t) = total_eta_text {
                            ui.label(
                                egui::RichText::new(format!("  {t}"))
                                    .size(11.0)
                                    .color(ui.visuals().warn_fg_color),
                            )
                            .on_hover_text(
                                "進行中インデクサ全体の残り時間と処理速度。\n\
                                 残り時間は最も時間がかかる索引のもの (並列実行)、\n\
                                 速度は全索引の合計。",
                            );
                        }
                    });
                    if active.is_empty() {
                        ui.label(
                            egui::RichText::new("  (アイドル)")
                                .size(11.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                    } else {
                        for (name, msg) in &active {
                            ui.label(
                                egui::RichText::new(format!(
                                    "  {}: {}",
                                    name,
                                    truncate_name(msg, 100),
                                ))
                                .size(11.0)
                                .monospace()
                                .color(ui.visuals().weak_text_color()),
                            )
                            .on_hover_text(format!("{name}: {msg}"));
                        }
                    }
                    // ライブ更新: 100ms ごとに再描画を要求して進捗を流す。
                    // active が空でも notify-rs が動き出した瞬間に拾えるよう常に呼ぶ。
                    ctx.request_repaint_after(Duration::from_millis(100));
                }

                if escape_pressed && self.favorite_delete_confirm.is_none() {
                    close_requested = true;
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("閉じる").clicked() {
                        close_requested = true;
                    }
                });
                    });
            });

        // お気に入り編集には「現在のページ」が無い。ON の種と OFF 後の比較先は
        // どちらも共通標準に固定する。
        for (favorite_id, favorite_name, enabled) in favorite_default_toggles {
            if enabled {
                let seed = self.settings.global_preset.clone();
                self.create_favorite_specific_default(favorite_id, &favorite_name, seed);
            } else {
                let fallback = self.settings.global_preset.clone();
                self.request_favorite_specific_default_clear(favorite_id, favorite_name, fallback);
            }
        }

        // ── 削除確認 ────────────────────────────────────────────────
        if let Some(fav_id) = self.favorite_delete_confirm {
            let target = self
                .settings
                .favorites
                .iter()
                .find(|fav| fav.id == fav_id)
                .map(|fav| (fav.name.clone(), fav.path.clone()));
            if let Some((name, path)) = target {
                let mut close_confirm = false;
                let mut confirmed = false;
                let response = egui::Modal::new(egui::Id::new("favorite_delete_confirm_modal"))
                    .show(ctx, |ui| {
                        ui.set_min_width(420.0);
                        ui.heading("お気に入りを削除");
                        ui.add_space(8.0);
                        ui.label(format!("「{name}」をお気に入りから削除します。"));
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(path.to_string_lossy())
                                .monospace()
                                .weak(),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(
                                "コンテナ索引・アイテム索引・お気に入り標準設定も解除されます。",
                            )
                            .weak(),
                        );
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button("削除").clicked() {
                                confirmed = true;
                                close_confirm = true;
                            }
                            if ui.button("キャンセル").clicked() {
                                close_confirm = true;
                            }
                        });
                    });
                if confirmed {
                    remove = self
                        .settings
                        .favorites
                        .iter()
                        .position(|fav| fav.id == fav_id);
                }
                if close_confirm || response.should_close() {
                    self.favorite_delete_confirm = None;
                }
            } else {
                self.favorite_delete_confirm = None;
            }
        }

        // ── スワップ / 削除 / フラグ変更を反映 ──
        // 並び替えは副作用なし (UI 順の入れ替えだけ)。削除は supervisor drop と
        // 索引データのクリーンアップが必要。
        if let Some((a, b)) = swap {
            self.settings.favorites.swap(a, b);
            any_setting_dirty = true;
        }
        if let Some(i) = remove {
            // 削除対象の favorite の索引データもクリーンアップする。
            // (フラグが OFF なら副作用なしなので害はない。ON→削除でも正しく掃除される。)
            let removed = self.settings.favorites.remove(i);
            if removed.auto_index_structure {
                name_index_toggles.push((removed.id, removed.path.clone(), false));
            }
            if removed.auto_index_metadata {
                meta_index_toggles.push((removed.id, false));
            }
            // 補正のお気に入り標準も即時に掃除する (次回起動時の prune_favorite_params
            // を待たない)。これで削除直後にフォルダを再訪したとき、残像の favorite 標準が
            // 効いたまま、という不整合を避ける。
            self.clear_favorite_default(removed.id);
            any_setting_dirty = true;
        }

        // ── フラグ変更の副作用をまとめて実行 ──
        // 名前索引: true→false は DB 即クリア、false→true は bulk 起動 (冪等)
        for (fav_id, path, new_on) in &name_index_toggles {
            self.apply_favorite_name_index_change(*fav_id, path, *new_on);
        }
        // メタ索引: 副作用は内部で sync_with_favorites を呼ぶので、複数変更があれば
        // 最後に 1 回呼ばれる形でよい。ただし OFF→ON で supervisor を spawn する
        // タイミングは 1 回で十分なので最後の 1 つだけ反映してもよいが、素直に
        // ループしてもコストは少ない (sync はほぼ idempotent)。
        for (fav_id, new_on) in &meta_index_toggles {
            self.apply_favorite_meta_index_change(*fav_id, *new_on);
        }

        // 並び替え / 削除 / 名前編集のみだった場合も save を走らせる
        if any_setting_dirty {
            self.settings.save();
            // 「常駐中はインデックス更新を一時停止する」が変わっていたら、トレイメニューの
            // checkmark も同期する (環境設定ダイアログの同項目と同じ経路)。
            if old_pause_minimized != self.settings.pause_indexer_while_minimized {
                self.sync_tray_pause_check();
            }
        }

        if close_requested || !open {
            self.show_favorites_editor = false;
            self.favorite_delete_confirm = None;
            self.favorites_index_size_cache = None;
            self.favorites_index_count_cache = None;
            // 走行中の worker は drop で rx を切るだけ。worker は send 後に
            // 自然終了するので join は不要 (重い処理は DB COUNT のみ)。
            self.favorites_index_refresh_rx = None;
            self.favorites_total_eta_cache = None;
        }

        // サムネ一括作成ダイアログを起動 (「お気に入り」ダイアログと同じフレームで連続
        // 操作可能にするため、閉じる/閉じない は問わず後段で呼ぶ)。
        if open_cache_creator {
            self.cc.checked = vec![false; self.settings.favorites.len()];
            self.cc.running = false;
            self.cc.result = None;
            self.cc.total.store(0, std::sync::atomic::Ordering::Relaxed);
            self.cc.done.store(0, std::sync::atomic::Ordering::Relaxed);
            self.cc
                .cache_size
                .store(0, std::sync::atomic::Ordering::Relaxed);
            self.cc
                .finished
                .store(false, std::sync::atomic::Ordering::Relaxed);
            *self.cc.current.lock().unwrap() = String::new();
            self.cc.show = true;
        }
    }
}

fn favorites_editor_dialog_geometry(
    content_rect: egui::Rect,
    has_favorites: bool,
) -> (egui::Rect, egui::Vec2, egui::Vec2) {
    let safe_rect = content_rect.shrink2(egui::vec2(16.0, 16.0));
    let safe_size = safe_rect.size();
    let preferred_height = if has_favorites { 800.0 } else { 420.0 };
    let dialog_size = egui::vec2(980.0, preferred_height).min(safe_size);
    let min_dialog_size = egui::vec2(760.0, 420.0).min(safe_size);
    (safe_rect, dialog_size, min_dialog_size)
}

/// 全 supervisor の `EtaSnapshot` を統合し、「残り XX:XX (NN件/秒)」表記を返す。
///
/// 各索引は **並列に** 走るので:
/// - 残り時間 = `max(各 remaining_secs)` (一番遅いやつが律速)
/// - 処理速度 = `Σ(各 rate_per_sec)` (合計スループット)
///
/// `remaining_secs` がまだ算出できていない (= サンプル不足) ものは除外する。
/// 全部除外で空集合になったら None。
fn aggregate_total_eta(etas: &[crate::indexer_progress::EtaSnapshot]) -> Option<String> {
    let max_remaining = etas.iter().filter_map(|e| e.remaining_secs).max()?;
    let total_rate: f64 = etas.iter().map(|e| e.rate_per_sec).sum();
    let hms = crate::indexer_progress::format_eta_hms(max_remaining);
    Some(format!("[残り {hms} ({:.0}件/秒)]", total_rate))
}

// ── チェックボックス右側の状態インライン表示ヘルパー ──────────────────────

/// 名前索引・メタ索引共通の状態表示。
///
/// `flags` は supervisor から得た `(in_full_scan, initial_scan_done)`。
/// `None` = supervisor 未登録 (= 起動直後)。
///
/// - `on=false`: —
/// - flags=None: ⏳ 起動中
/// - in_full_scan: ⏳ スキャン中
/// - initial_scan_done: ✅ 監視中
/// - else: ⏳ 準備中 (supervisor 起動済みだが scan 未着手)
///
/// `file_count > 0` のときは末尾に `(123,456件)` を付ける。
fn draw_state_inline(ui: &mut egui::Ui, on: bool, flags: Option<(bool, bool)>, file_count: u64) {
    if !on {
        ui.label(
            egui::RichText::new("—")
                .size(11.0)
                .color(ui.visuals().weak_text_color()),
        );
        return;
    }
    const YELLOW: egui::Color32 = egui::Color32::from_rgb(200, 170, 60);
    const GREEN: egui::Color32 = egui::Color32::from_rgb(100, 170, 100);
    let (label, color) = match flags {
        None => ("⏳ 起動中", YELLOW),
        Some((true, _)) => ("⏳ スキャン中", YELLOW),
        Some((false, true)) => ("✅ 監視中", GREEN),
        Some((false, false)) => ("⏳ 準備中", YELLOW),
    };
    let text = if file_count == 0 {
        label.to_string()
    } else {
        format!("{label} ({}件)", format_count(file_count))
    };
    ui.label(egui::RichText::new(text).size(11.0).color(color));
}

#[cfg(test)]
mod tests {
    use super::favorites_editor_dialog_geometry;

    #[test]
    fn favorites_editor_dialog_geometry_stays_inside_small_window() {
        let content_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 360.0));
        let (safe_rect, dialog_size, min_dialog_size) =
            favorites_editor_dialog_geometry(content_rect, true);

        assert_eq!(safe_rect.size(), egui::vec2(608.0, 328.0));
        assert_eq!(dialog_size, safe_rect.size());
        assert_eq!(min_dialog_size, safe_rect.size());
    }

    #[test]
    fn favorites_editor_dialog_geometry_uses_preferred_height_when_it_fits() {
        let content_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 1000.0));
        let (safe_rect, dialog_size, min_dialog_size) =
            favorites_editor_dialog_geometry(content_rect, true);

        assert_eq!(safe_rect.size(), egui::vec2(1248.0, 968.0));
        assert_eq!(dialog_size, egui::vec2(980.0, 800.0));
        assert_eq!(min_dialog_size, egui::vec2(760.0, 420.0));
    }
}
