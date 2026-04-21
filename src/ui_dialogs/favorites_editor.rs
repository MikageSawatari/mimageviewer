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

                if self.settings.favorites.is_empty() {
                    ui.label("お気に入りはまだ登録されていません。");
                    ui.add_space(4.0);
                } else {
                    ui.label(
                        egui::RichText::new(
                            "お気に入り配下を索引化して Ctrl+S / Ctrl+G で検索できます。\n\
                             チェックを入れた項目はこの場で 1 回全走査し、以降は\n\
                             notify-rs と起動時スキャンで自動更新します。",
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
                                .num_columns(6)
                                .spacing([8.0, 4.0])
                                .show(ui, |ui| {
                                    // ── ヘッダ ──
                                    ui.label(egui::RichText::new("表示名").strong());
                                    ui.label(egui::RichText::new("パス").strong());
                                    ui.label(egui::RichText::new("名前索引").strong())
                                        .on_hover_text(
                                            "フォルダ / ZIP / PDF / 画像のファイル名を Ctrl+S で検索",
                                        );
                                    ui.label(egui::RichText::new("メタデータ索引").strong())
                                        .on_hover_text(
                                            "AI プロンプト / EXIF / XMP を Ctrl+F / Ctrl+G で検索",
                                        );
                                    ui.label(egui::RichText::new("状態").strong()).on_hover_text(
                                        "メタ索引の初期スキャン状態\n\
                                         ✅ = 完了 / ⏳ = スキャン中",
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

                                        // メタ索引の状態 (supervisor がいれば ✅/⏳、いなければ —)
                                        let stats = stats_by_id.get(&fav_id);
                                        draw_state_cell(ui, meta_on, stats);

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
                    // 初期スキャン中 / ingest 中の supervisor の "今何してる" を
                    // 1 行ずつ列挙する。完了済み or アイドル supervisor は出さない。
                    // スキャン完了の瞬間に progress.clear() されるので、何も出ない=
                    // 全インデクサがアイドルという意味。
                    let active: Vec<(String, String)> = self
                        .settings
                        .favorites
                        .iter()
                        .filter_map(|fav| {
                            stats_by_id
                                .get(&fav.id)
                                .and_then(|s| s.current_activity.as_ref())
                                .map(|msg| (fav.name.clone(), msg.clone()))
                        })
                        .collect();
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

