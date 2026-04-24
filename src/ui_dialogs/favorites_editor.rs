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
use crate::ui_helpers::{format_bytes, truncate_name};

/// 全文検索インデックスの on-disk サイズ (bytes) を集計する。
///
/// - `name_index_db`: お気に入り単位の名前索引 (SQLite)
/// - `fts_meta_db`: メタ索引の per-source テキスト (SQLite、本体が 2GB 級になりうる)
/// - `fts_index_dir`: Tantivy インデックス (segment ファイル群の合計)
///
/// すべて `stat()` ベースなので数ミリ秒で完了する。ダイアログを開くたびに再計算して
/// 最新値を出す (ingest で増えるので)。存在しないファイル / ディレクトリは 0 扱い。
struct IndexDiskSizes {
    name_index_db: u64,
    fts_meta_db: u64,
    fts_index_dir: u64,
}

fn compute_index_disk_sizes() -> IndexDiskSizes {
    let data_dir = crate::data_dir::get();
    let file_size = |p: &std::path::Path| -> u64 {
        std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
    };
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
        let mut any_setting_dirty = false;
        let mut swap: Option<(usize, usize)> = None;
        let mut remove: Option<usize> = None;
        let mut open_cache_creator = false;
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        let escape_pressed = self.dialog_escape_pressed(ctx);

        // 事前にインデクサ情報を取り出し (borrow 競合回避)
        let startup_diag = self.indexer_manager.as_ref().map(|m| m.startup_diag());
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

        let scroll_max_h = (ctx.content_rect().height() - 260.0).min(640.0).max(120.0);
        egui::Window::new("お気に入り")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(760.0);

                // ── 起動時整合性チェック (インデクサが有効なら表示) ──
                if reconciling {
                    ui.label(
                        egui::RichText::new(
                            "📋 インデックス整合性チェック中… (起動時 reconciliation)",
                        )
                        .color(egui::Color32::from_rgb(200, 170, 60))
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
                                    .color(egui::Color32::from_gray(150)),
                            )
                            .on_hover_text(
                                "環境設定「インデクサの速度プロファイル」で変更可。\n\
                                 1=省電力 / 2=標準 / 4=高速",
                            );
                        });

                        // インデックスのディスク使用量。ダイアログ open 時に 1 回計算してキャッシュ
                        // (毎フレーム stat + read_dir を叩かないため。Codex P3 指摘)。
                        // ダイアログ close 時に None に戻す。
                        let sizes = self
                            .favorites_index_size_cache
                            .get_or_insert_with(|| {
                                let s = compute_index_disk_sizes();
                                (s.name_index_db, s.fts_meta_db, s.fts_index_dir)
                            });
                        let (name_size, fts_meta_size, fts_index_size) = *sizes;
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("💾 インデックスサイズ:")
                                    .size(11.0)
                                    .color(egui::Color32::from_gray(150)),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "名前索引 {}",
                                    format_bytes(name_size)
                                ))
                                .size(11.0)
                                .color(egui::Color32::from_gray(150)),
                            )
                            .on_hover_text(
                                "search_index.db (名前索引) のディスク使用量 (WAL/SHM 込み)。\n\
                                 お気に入りごとの ファイル / フォルダ名インデックス。",
                            );
                            ui.separator();
                            ui.label(
                                egui::RichText::new(format!(
                                    "メタ索引 {} + {}",
                                    format_bytes(fts_meta_size),
                                    format_bytes(fts_index_size)
                                ))
                                .size(11.0)
                                .color(egui::Color32::from_gray(150)),
                            )
                            .on_hover_text(
                                "メタ索引のディスク使用量 (WAL/SHM 込み)。\n\
                                 ・fts_meta.db: 正規化済みの検索対象テキスト (EXIF / XMP /\n\
                                   PNG AI プロンプト / タグ) を格納。大量画像で数 GB になる\n\
                                 ・fts_index/: Tantivy 全文検索インデックス (バイグラム索引)",
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
                            "💡 アプリケーションを終了すると、次回起動時に\n\
                             インデックスの再スキャンが行われます。\n\
                             終了する代わりにタスクトレイに常駐すると、\n\
                             起動がスムーズになります。",
                        )
                        .size(11.0)
                        .color(egui::Color32::from_gray(180)),
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
                            "お気に入りは以下を索引化して、名前検索 (Ctrl+S) ・\n\
                             メタデータ検索 (Ctrl+G) できます。\n\
                             チェックを入れた項目はこの場で 1 回全走査し、以降は\n\
                             ファイルの変更監視と起動時スキャンで自動更新します。",
                        )
                        .weak()
                        .size(11.0),
                    );
                    ui.add_space(6.0);

                    let n = self.settings.favorites.len();
                    egui::ScrollArea::vertical()
                        .id_salt("fav_edit_scroll")
                        .max_height(scroll_max_h)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            egui::Grid::new("fav_edit_grid")
                                .striped(true)
                                .num_columns(5)
                                .spacing([8.0, 4.0])
                                .show(ui, |ui| {
                                    // ── ヘッダ (6→5 列に圧縮、状態は各索引列にインライン) ──
                                    ui.label(egui::RichText::new("表示名").strong());
                                    ui.label(egui::RichText::new("パス").strong());
                                    ui.label(egui::RichText::new("名前索引 (Ctrl+S)").strong())
                                        .on_hover_text(
                                            "フォルダ / ZIP / PDF / 画像のファイル名を検索\n\
                                             ✅ = 索引あり / ⏳ = バルク作成中",
                                        );
                                    ui.label(
                                        egui::RichText::new("メタデータ索引 (Ctrl+F / Ctrl+G)")
                                            .strong(),
                                    )
                                    .on_hover_text(
                                        "AI プロンプト / EXIF / XMP を検索\n\
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
                                            draw_name_state_inline(
                                                ui,
                                                name_on,
                                                name_stats_by_id.get(&fav_id),
                                            );
                                        });

                                        // メタデータ索引: チェック + 状態インライン
                                        ui.horizontal(|ui| {
                                            let meta_resp = ui.checkbox(
                                                &mut self.settings.favorites[i]
                                                    .auto_index_metadata,
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
                                            draw_meta_state_inline(
                                                ui,
                                                meta_on,
                                                stats_by_id.get(&fav_id),
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
                                                remove = Some(i);
                                            }
                                        });
                                        ui.end_row();
                                    }
                                });
                        });

                    // ── 一括操作 (チェックボックスの一括 ON/OFF のみ) ──
                    // 変化した favorite だけ toggle リストに積んで、ループ後にまとめて
                    // 副作用 (supervisor sync / name bulk 起動 / cleanup) を走らせる。
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("一括操作:").weak());
                        if ui.button("名前 全ON").clicked() {
                            for f in &mut self.settings.favorites {
                                if !f.auto_index_structure {
                                    f.auto_index_structure = true;
                                    name_index_toggles.push((f.id, f.path.clone(), true));
                                    any_setting_dirty = true;
                                }
                            }
                        }
                        if ui.button("名前 全OFF").clicked() {
                            for f in &mut self.settings.favorites {
                                if f.auto_index_structure {
                                    f.auto_index_structure = false;
                                    name_index_toggles.push((f.id, f.path.clone(), false));
                                    any_setting_dirty = true;
                                }
                            }
                        }
                        if ui.button("メタ 全ON").clicked() {
                            for f in &mut self.settings.favorites {
                                if !f.auto_index_metadata {
                                    f.auto_index_metadata = true;
                                    meta_index_toggles.push((f.id, true));
                                    any_setting_dirty = true;
                                }
                            }
                        }
                        if ui.button("メタ 全OFF").clicked() {
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
                    // まとめて列挙する。完了済み or アイドルは出さない。
                    // 完了の瞬間に progress.clear() されるので、何も出ない=全アイドル。
                    let mut active: Vec<(String, String)> = Vec::new();
                    for fav in &self.settings.favorites {
                        // メタ index (indexer_manager supervisor 経由)
                        if let Some(msg) = stats_by_id
                            .get(&fav.id)
                            .and_then(|s| s.current_activity.as_ref())
                        {
                            active.push((format!("{} (メタ)", fav.name), msg.clone()));
                        }
                        // 名前 index (name_index_supervisors 経由)
                        if let Some(msg) = name_stats_by_id
                            .get(&fav.id)
                            .and_then(|s| s.current_activity.as_ref())
                        {
                            active.push((format!("{} (名前)", fav.name), msg.clone()));
                        }
                    }
                    if !active.is_empty() {
                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new("🔄 バックグラウンドインデクサ")
                                .size(11.0)
                                .color(egui::Color32::from_rgb(200, 170, 60)),
                        );
                        for (name, msg) in &active {
                            ui.label(
                                egui::RichText::new(format!(
                                    "  {}: {}",
                                    name,
                                    truncate_name(msg, 100)
                                ))
                                .size(11.0)
                                .monospace()
                                .color(egui::Color32::from_gray(150)),
                            )
                            .on_hover_text(format!("{name}: {msg}"));
                        }
                        // ライブ更新: 100ms ごとに再描画を要求して進捗を流す
                        ctx.request_repaint_after(Duration::from_millis(100));
                    }
                }

                if escape_pressed {
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
            // pause_indexer_while_minimized が変わったかもしれないので、トレイメニューの
            // checkmark を設定値に合わせて同期する。他の項目 (名前・並び替え等) でも
            // 毎回 push するが、`SetPausedCheck` は idempotent で cost も低いので許容。
            if let Some(tc) = &self.tray_controller {
                tc.set_paused_check(self.settings.pause_indexer_while_minimized);
            }
        }

        if close_requested || !open {
            self.show_favorites_editor = false;
            // 次回 open 時に最新のディスク使用量で再計算するためキャッシュを破棄。
            self.favorites_index_size_cache = None;
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

// ── チェックボックス右側の状態インライン表示ヘルパー ──────────────────────

/// 名前索引列: メタ索引と同じ 3 分岐で表示する
/// (両方とも notify-rs 監視を張って差分追従する構造になったため)。
///
/// - OFF: —
/// - ON + `in_full_scan=true`: ⏳ スキャン中
/// - ON + `initial_scan_done=true`, `in_full_scan=false`: ✅ 監視中
/// - ON + supervisor 未登録 / `initial_scan_done=false`: ⏳ 起動中
fn draw_name_state_inline(
    ui: &mut egui::Ui,
    on: bool,
    stats: Option<&crate::name_index_supervisor::NameIndexStats>,
) {
    if !on {
        ui.label(
            egui::RichText::new("—")
                .size(11.0)
                .color(egui::Color32::from_gray(120)),
        );
        return;
    }
    let Some(s) = stats else {
        ui.label(
            egui::RichText::new("⏳ 起動中")
                .size(11.0)
                .color(egui::Color32::from_rgb(200, 170, 60)),
        );
        return;
    };
    if s.in_full_scan {
        ui.label(
            egui::RichText::new("⏳ スキャン中")
                .size(11.0)
                .color(egui::Color32::from_rgb(200, 170, 60)),
        );
    } else if s.initial_scan_done {
        ui.label(
            egui::RichText::new("✅ 監視中")
                .size(11.0)
                .color(egui::Color32::from_rgb(100, 170, 100)),
        );
    } else {
        ui.label(
            egui::RichText::new("⏳ 準備中")
                .size(11.0)
                .color(egui::Color32::from_rgb(200, 170, 60)),
        );
    }
}

/// メタデータ索引列: supervisor 状態を 3 分岐で表示
///
/// - OFF: —
/// - ON + `in_full_scan=true`: ⏳ スキャン中 (walker + ingest 実行中)
/// - ON + `initial_scan_done=true`, `in_full_scan=false`: ✅ 監視中
///   (notify-rs watcher が FS 変更を待機、scan 完了後アイドル状態)
/// - ON + `initial_scan_done=false`, `in_full_scan=false`: ⏳ 準備中
///   (supervisor が起動したばかりで scan がまだ始まっていない)
fn draw_meta_state_inline(ui: &mut egui::Ui, on: bool, stats: Option<&SupervisorStats>) {
    if !on {
        ui.label(
            egui::RichText::new("—")
                .size(11.0)
                .color(egui::Color32::from_gray(120)),
        );
        return;
    }
    let Some(s) = stats else {
        ui.label(
            egui::RichText::new("⏳ 起動中")
                .size(11.0)
                .color(egui::Color32::from_rgb(200, 170, 60)),
        );
        return;
    };
    if s.in_full_scan {
        ui.label(
            egui::RichText::new("⏳ スキャン中")
                .size(11.0)
                .color(egui::Color32::from_rgb(200, 170, 60)),
        );
    } else if s.initial_scan_done {
        ui.label(
            egui::RichText::new("✅ 監視中")
                .size(11.0)
                .color(egui::Color32::from_rgb(100, 170, 100)),
        );
    } else {
        ui.label(
            egui::RichText::new("⏳ 準備中")
                .size(11.0)
                .color(egui::Color32::from_rgb(200, 170, 60)),
        );
    }
}

