//! 「お気に入り」ダイアログ (v0.8.0 で旧 `favorites_editor` + `index_manager` を統合)。
//!
//! 1 つのダイアログで:
//! - 表示名 / 並べ替え / 削除
//! - 3 種の自動処理の ON/OFF (名前索引 / メタ索引 / サムネイルキャッシュ)
//! - 起動時整合性チェックの所要時間
//! - お気に入り単位の初期スキャン時間・取込/削除件数・エラー診断
//! - 手動での再構築
//!
//! を扱う。旧 2 つのダイアログを別にしていた頃は「フラグは A で切り替え、状態は B で
//! 見る」という導線で分かりにくかったため 1 本化した。

use eframe::egui;

use crate::app::App;
use crate::indexer_supervisor::SupervisorStats;
use crate::ui_helpers::truncate_name;

impl App {
    pub(crate) fn show_favorites_editor_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_favorites_editor {
            return;
        }
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let mut swap: Option<(usize, usize)> = None;
        let mut remove: Option<usize> = None;
        let mut request_rescan_for: Option<uuid::Uuid> = None;
        let mut request_rescan_all = false;
        let mut open_index_creator = false;
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

        let scroll_max_h = (ctx.content_rect().height() - 260.0).min(640.0).max(120.0);
        egui::Window::new("お気に入り")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(1100.0);

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
                                egui::RichText::new(format!(
                                    "pending 整理 {} / tombstone 消去 {}",
                                    diag.pending_cleaned, diag.tombstone_purged
                                ))
                                .size(11.0)
                                .color(egui::Color32::from_gray(150)),
                            )
                            .on_hover_text(
                                "前回異常終了などで status != ok で残った行を整理した件数。\n\
                                 通常はどちらも 0 件。",
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
                    });
                    ui.add_space(4.0);
                }

                // ── 説明 (3 種のフラグの意味) ──
                if self.settings.favorites.is_empty() {
                    ui.label("お気に入りはまだ登録されていません。");
                    ui.add_space(4.0);
                } else {
                    ui.label(
                        egui::RichText::new(
                            "お気に入り配下をフル索引化するかを選べます。\n\
                             ・名前索引 — フォルダ / ZIP / PDF 名を Ctrl+S で検索\n\
                             ・メタ索引 — AI プロンプト / EXIF / XMP など画像メタを Ctrl+F / Ctrl+G で検索\n\
                             チェックを入れた項目はこの場で 1 回全走査し、以降は notify-rs と\n\
                             起動時スキャンで自動更新します。未チェックでも、閲覧したフォルダは\n\
                             軽い索引追記 (名前) が走ります。\n\
                             サムネイルは I/O が重い (GB 規模) ため自動化から外し、\n\
                             下部の「🖼 サムネを一括作成」ボタンで手動起動してください。",
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
                                .num_columns(9)
                                .spacing([8.0, 4.0])
                                .show(ui, |ui| {
                                    // ── ヘッダ ──
                                    ui.label(egui::RichText::new("表示名").strong());
                                    ui.label(egui::RichText::new("パス").strong());
                                    ui.label(egui::RichText::new("名前").strong()).on_hover_text(
                                        "名前索引を全走査で作る + 以降自動更新する。\n\
                                         OFF でも閲覧したフォルダは軽く追記される (Ctrl+S 用)。",
                                    );
                                    ui.label(egui::RichText::new("メタ").strong()).on_hover_text(
                                        "メタ索引を全走査で作る + 以降自動更新する。\n\
                                         画像メタ全文検索 (Ctrl+F / Ctrl+G) で使う。\n\
                                         OFF でも閲覧フォルダの軽い追記が将来入る予定 (v0.8.x)。",
                                    );
                                    ui.label(egui::RichText::new("状態").strong()).on_hover_text(
                                        "メタ索引の初期スキャン状態\n\
                                         ✅ = 完了 / ⏳ = スキャン中",
                                    );
                                    ui.label(egui::RichText::new("スキャン").strong())
                                        .on_hover_text(
                                            "メタ索引の最新スキャン所要時間 / 走査ファイル数。\n\
                                             初期スキャンや手動再構築の値が反映される。",
                                        );
                                    ui.label(egui::RichText::new("取込").strong()).on_hover_text(
                                        "本セッションのメタ索引 ingest 累積 (ok / failed)",
                                    );
                                    ui.label(egui::RichText::new("削除").strong()).on_hover_text(
                                        "本セッションの削除検出件数",
                                    );
                                    ui.label(egui::RichText::new("操作").strong());
                                    ui.end_row();

                                    // ── 各行 ──
                                    for i in 0..n {
                                        let fav_id = self.settings.favorites[i].id;
                                        let meta_on = self.settings.favorites[i].auto_index_metadata;

                                        // 表示名 (編集可能)
                                        let _ = ui.add_sized(
                                            [100.0, 20.0],
                                            egui::TextEdit::singleline(
                                                &mut self.settings.favorites[i].name,
                                            ),
                                        );

                                        // パス (読み取り専用)
                                        let path_str = self.settings.favorites[i]
                                            .path
                                            .to_string_lossy()
                                            .to_string();
                                        ui.label(
                                            egui::RichText::new(truncate_name(&path_str, 40))
                                                .monospace()
                                                .weak(),
                                        )
                                        .on_hover_text(&path_str);

                                        // 2 種のフル索引化フラグ (サムネは手動バルクのみ)
                                        ui.checkbox(
                                            &mut self.settings.favorites[i].auto_index_structure,
                                            "",
                                        );
                                        ui.checkbox(
                                            &mut self.settings.favorites[i].auto_index_metadata,
                                            "",
                                        );

                                        // メタ索引の統計 (supervisor がいれば表示、いなければ —)
                                        let stats = stats_by_id.get(&fav_id);
                                        draw_state_cell(ui, meta_on, stats);
                                        draw_scan_cell(ui, meta_on, stats);
                                        draw_ingest_cell(ui, meta_on, stats);
                                        draw_deleted_cell(ui, meta_on, stats);

                                        // 操作
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
                                            // メタ索引が有効なお気に入りだけ再構築可能
                                            let rescan_enabled = meta_on && stats.is_some();
                                            if ui
                                                .add_enabled(
                                                    rescan_enabled,
                                                    egui::Button::new("🔄"),
                                                )
                                                .on_hover_text(
                                                    "メタ索引をこのお気に入りで再走査する",
                                                )
                                                .clicked()
                                            {
                                                request_rescan_for = Some(fav_id);
                                            }
                                            if ui.button("削除").clicked() {
                                                remove = Some(i);
                                            }
                                        });
                                        ui.end_row();
                                    }
                                });
                        });

                    // ── 一括操作 ──
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("一括操作:").weak());
                        if ui.button("名前 全ON").clicked() {
                            for f in &mut self.settings.favorites {
                                f.auto_index_structure = true;
                            }
                        }
                        if ui.button("名前 全OFF").clicked() {
                            for f in &mut self.settings.favorites {
                                f.auto_index_structure = false;
                            }
                        }
                        if ui.button("メタ 全ON").clicked() {
                            for f in &mut self.settings.favorites {
                                f.auto_index_metadata = true;
                            }
                        }
                        if ui.button("メタ 全OFF").clicked() {
                            for f in &mut self.settings.favorites {
                                f.auto_index_metadata = false;
                            }
                        }
                    });

                    // ── バルク一括実行ボタン群 (旧メニュー統合) ──
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button("🔄 メタ索引をすべて再構築")
                            .on_hover_text(
                                "メタ索引が有効なお気に入りをすべて再走査する",
                            )
                            .clicked()
                        {
                            request_rescan_all = true;
                        }
                        if ui
                            .button("📂 名前索引を一括作成…")
                            .on_hover_text(
                                "名前索引ダイアログを開きます。お気に入り配下全体を一括\n\
                                 スキャンして Ctrl+S 検索用の索引を構築します。",
                            )
                            .clicked()
                        {
                            open_index_creator = true;
                        }
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
                }

                if escape_pressed {
                    cancel = true;
                }

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

        if let Some((a, b)) = swap {
            self.settings.favorites.swap(a, b);
        }
        if let Some(i) = remove {
            self.settings.favorites.remove(i);
        }

        // 再構築リクエストは OK/キャンセル前でもそのまま supervisor に送る。
        // チェックボックス変更はまだ apply されていないが、現 supervisor は動いている
        // ので再構築自体は意味がある (キャンセルしても再構築は完了する)。
        if let Some(mgr) = self.indexer_manager.as_ref() {
            if let Some(id) = request_rescan_for {
                mgr.request_full_rescan(id);
            }
            if request_rescan_all {
                for (id, _) in &stats_by_id {
                    mgr.request_full_rescan(*id);
                }
            }
        }

        if apply {
            self.settings.save();
            // 編集前後のフラグ差分から「false→true で bulk 起動」「true→false で索引削除」を決める。
            // snapshot に無い favorite は新規追加なので、fav_add 側で起動済み (TODO: fav_add 側でまだ bulk 呼ばれていない)。
            let transitions = self.compute_favorite_index_transitions();
            self.apply_favorite_index_cleanup(&transitions);
            // インデクサに反映: ON/OFF 切り替えで Supervisor を spawn/stop する。
            if let Some(mgr) = self.indexer_manager.as_mut() {
                mgr.sync_with_favorites(&self.settings.favorites);
            }
            self.apply_favorite_index_bulk_start(&transitions);
            self.show_favorites_editor = false;
        } else if cancel || !open {
            self.settings = crate::settings::Settings::load();
            self.favorites_pre_edit_snapshot = None;
            self.show_favorites_editor = false;
        }

        // バルク実行ダイアログを起動 (「お気に入り」ダイアログと同じフレームで連続
        // 操作可能にするため、閉じる/閉じない は問わず後段で呼ぶ)。
        if open_index_creator {
            self.ic.checked = self
                .settings
                .favorites
                .iter()
                .map(|fav| {
                    self.settings
                        .search_index_checks
                        .iter()
                        .any(|p| p == &fav.path)
                })
                .collect();
            self.ic.reset_for_open();
            self.ic.show = true;
        }
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

// ── セル描画ヘルパー (メタ索引の supervisor 統計) ─────────────────────────────

fn draw_state_cell(ui: &mut egui::Ui, meta_on: bool, stats: Option<&SupervisorStats>) {
    match (meta_on, stats) {
        (true, Some(s)) => {
            if s.initial_scan_done {
                ui.label(
                    egui::RichText::new("✅ 完了")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(100, 170, 100)),
                );
            } else {
                ui.label(
                    egui::RichText::new("⏳ スキャン中")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(200, 170, 60)),
                );
            }
        }
        _ => {
            ui.label(
                egui::RichText::new("—")
                    .size(11.0)
                    .color(egui::Color32::from_gray(120)),
            );
        }
    }
}

fn draw_scan_cell(ui: &mut egui::Ui, meta_on: bool, stats: Option<&SupervisorStats>) {
    match (meta_on, stats) {
        (true, Some(s)) => {
            let text = match s.last_scan_duration_ms {
                Some(ms) => format!("{} ms / {} 件", ms, s.last_scan_total_scanned),
                None => "—".to_string(),
            };
            let diag = &s.last_scan_diag;
            let has_errors = diag.read_dir_errors > 0
                || diag.file_type_errors > 0
                || diag.metadata_errors > 0
                || diag.depth_limit_hits > 0;
            let color = if has_errors {
                egui::Color32::from_rgb(200, 120, 60)
            } else {
                egui::Color32::from_gray(160)
            };
            let mut hover = format!(
                "直近スキャン: {} ms\n走査ファイル数: {}",
                s.last_scan_duration_ms.unwrap_or(0),
                s.last_scan_total_scanned,
            );
            if let Some(init_ms) = s.initial_scan_duration_ms {
                hover.push_str(&format!("\n初期スキャン: {init_ms} ms"));
            }
            if has_errors {
                hover.push_str(&format!(
                    "\n\nエラー診断:\n\
                     \u{3000}read_dir 失敗: {}\n\
                     \u{3000}file_type 失敗: {}\n\
                     \u{3000}metadata 失敗: {}\n\
                     \u{3000}深さ上限到達: {}",
                    diag.read_dir_errors,
                    diag.file_type_errors,
                    diag.metadata_errors,
                    diag.depth_limit_hits,
                ));
            }
            ui.label(egui::RichText::new(text).size(11.0).color(color))
                .on_hover_text(hover);
        }
        _ => {
            ui.label(
                egui::RichText::new("—")
                    .size(11.0)
                    .color(egui::Color32::from_gray(120)),
            );
        }
    }
}

fn draw_ingest_cell(ui: &mut egui::Ui, meta_on: bool, stats: Option<&SupervisorStats>) {
    match (meta_on, stats) {
        (true, Some(s)) => {
            let text = if s.ingested_failed > 0 {
                format!("{} (失敗 {})", s.ingested_ok, s.ingested_failed)
            } else {
                format!("{}", s.ingested_ok)
            };
            let color = if s.ingested_failed > 0 {
                egui::Color32::from_rgb(200, 120, 60)
            } else {
                egui::Color32::from_gray(160)
            };
            ui.label(egui::RichText::new(text).size(11.0).color(color));
        }
        _ => {
            ui.label(
                egui::RichText::new("—")
                    .size(11.0)
                    .color(egui::Color32::from_gray(120)),
            );
        }
    }
}

fn draw_deleted_cell(ui: &mut egui::Ui, meta_on: bool, stats: Option<&SupervisorStats>) {
    match (meta_on, stats) {
        (true, Some(s)) => {
            ui.label(
                egui::RichText::new(format!("{}", s.deleted))
                    .size(11.0)
                    .color(egui::Color32::from_gray(160)),
            );
        }
        _ => {
            ui.label(
                egui::RichText::new("—")
                    .size(11.0)
                    .color(egui::Color32::from_gray(120)),
            );
        }
    }
}
