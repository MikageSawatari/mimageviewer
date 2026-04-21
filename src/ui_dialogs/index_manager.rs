//! 「インデックス管理」ダイアログ (v0.8.0)。
//!
//! お気に入り単位で自動インデックスの状態 (structure / metadata / thumbnail) を
//! 一覧表示し、手動で再構築 (`request_full_rescan`) を要求できる統合 UI。
//!
//! 旧 `index_creator` (Ctrl+S 用フォルダ名索引の一括作成) / `cache_creator`
//! (サムネイル一括キャッシュ) は互換のため残してあるが、v0.8.0 以降はこちらを
//! 推奨する (docs/search-expansion-design.md §8.4)。
//!
//! ## 表示項目
//!
//! - お気に入り名 / パス
//! - メタデータ indexed 件数 (fts_meta.db の status=ok 件数)
//! - pending / failed の件数 (エラー診断表示)
//! - ingest_ok / ingested_failed / deleted の累積
//! - "再構築" ボタン — Supervisor に手動 FullRescan コマンドを送る
//! - reconciliation 中インジケータ (起動直後の数百 ms)
//!
//! 各 SupervisorStatsView は軽量な Mutex ロックで取得するため、毎フレーム呼んでも
//! 問題ない (Codex round-8 で確認済み)。

use eframe::egui;

use crate::app::App;
use crate::ui_helpers::truncate_name;

impl App {
    pub(crate) fn show_index_manager_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_index_manager {
            return;
        }
        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

        // まず一気にデータを取ってから UI を組む (borrow 競合を避ける)
        let manager_available = self.indexer_manager.is_some();
        let reconciliation_in_progress = self
            .indexer_manager
            .as_ref()
            .is_some_and(|m| m.is_reconciling());
        let startup_diag = self
            .indexer_manager
            .as_ref()
            .map(|m| m.startup_diag());
        let mut stats_list = self
            .indexer_manager
            .as_ref()
            .map(|mgr| mgr.all_stats())
            .unwrap_or_default();
        // お気に入りの表示順 (settings.favorites の順) に合わせる
        let id_order: std::collections::HashMap<uuid::Uuid, usize> = self
            .settings
            .favorites
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id, i))
            .collect();
        stats_list.sort_by_key(|v| id_order.get(&v.favorite_id).copied().unwrap_or(usize::MAX));

        let mut request_rescan_for: Option<uuid::Uuid> = None;
        let mut request_rescan_all = false;
        let mut close_requested = false;

        egui::Window::new("インデックス管理")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(920.0);

                if !manager_available {
                    ui.label(
                        egui::RichText::new(
                            "全文検索インデクサが利用できません (DB 初期化失敗)。\n\
                             Ctrl+G 検索は無効ですが、他の機能は通常どおり動作します。",
                        )
                        .weak(),
                    );
                    return;
                }

                if reconciliation_in_progress {
                    ui.label(
                        egui::RichText::new(
                            "📋 インデックス整合性チェック中… (起動時 reconciliation)",
                        )
                        .color(egui::Color32::from_rgb(200, 170, 60))
                        .size(11.0),
                    );
                    ui.add_space(4.0);
                }

                // 起動時診断セクション (reconciliation のコストが分かる)
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
                                egui::RichText::new(format!(
                                    "I/O 並列度: {}",
                                    diag.io_permits
                                ))
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

                ui.label(
                    egui::RichText::new(
                        "お気に入り単位で自動インデックスの状態を確認・再構築できます。\n\
                         「メタデータインデックス」が有効なお気に入りのみ表示されます。\n\
                         フラグの切り替えは「お気に入りの編集」ダイアログから行ってください。",
                    )
                    .weak()
                    .size(11.0),
                );
                ui.add_space(6.0);

                if stats_list.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "（メタデータインデックスが有効なお気に入りはまだありません）",
                        )
                        .weak(),
                    );
                    return;
                }

                egui::ScrollArea::vertical()
                    .id_salt("idx_mgr_scroll")
                    .max_height(480.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Grid::new("idx_mgr_grid")
                            .striped(true)
                            .num_columns(7)
                            .spacing([10.0, 4.0])
                            .show(ui, |ui| {
                                // ヘッダ
                                ui.label(egui::RichText::new("表示名").strong());
                                ui.label(egui::RichText::new("パス").strong());
                                ui.label(egui::RichText::new("状態").strong())
                                    .on_hover_text(
                                        "初期スキャン完了 = ✅ / 未完了 = ⏳",
                                    );
                                ui.label(egui::RichText::new("スキャン").strong())
                                    .on_hover_text(
                                        "最新スキャンの所要時間 / 走査ファイル数。\n\
                                         初期スキャンに加えて手動再構築・watcher overflow\n\
                                         による全再走査の値も反映される。",
                                    );
                                ui.label(egui::RichText::new("取込").strong())
                                    .on_hover_text(
                                        "本セッションの ingest 累積 (ok / failed)",
                                    );
                                ui.label(egui::RichText::new("削除").strong())
                                    .on_hover_text(
                                        "本セッションの削除検出件数",
                                    );
                                ui.label(egui::RichText::new("操作").strong());
                                ui.end_row();

                                for v in &stats_list {
                                    ui.label(
                                        egui::RichText::new(&v.favorite_name).monospace(),
                                    );
                                    // Cow<str> を保持して、UTF-8 パスの場合に String 追加アロケを避ける
                                    let path_str = v.favorite_path.to_string_lossy();
                                    ui.label(
                                        egui::RichText::new(truncate_name(&path_str, 50))
                                            .monospace()
                                            .weak(),
                                    )
                                    .on_hover_text(path_str.as_ref());
                                    // 状態
                                    if v.stats.initial_scan_done {
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
                                    // スキャン時間 (直近): 走査数と所要時間、およびエラー診断
                                    let scan_text = match v.stats.last_scan_duration_ms {
                                        Some(ms) => format!(
                                            "{} ms / {} 件",
                                            ms, v.stats.last_scan_total_scanned
                                        ),
                                        None => "—".to_string(),
                                    };
                                    let diag = &v.stats.last_scan_diag;
                                    let has_errors = diag.read_dir_errors > 0
                                        || diag.file_type_errors > 0
                                        || diag.metadata_errors > 0
                                        || diag.depth_limit_hits > 0;
                                    let scan_color = if has_errors {
                                        egui::Color32::from_rgb(200, 120, 60)
                                    } else {
                                        egui::Color32::from_gray(160)
                                    };
                                    let mut hover = format!(
                                        "直近スキャン: {} ms\n走査ファイル数: {}",
                                        v.stats.last_scan_duration_ms.unwrap_or(0),
                                        v.stats.last_scan_total_scanned,
                                    );
                                    if let Some(init_ms) = v.stats.initial_scan_duration_ms {
                                        hover
                                            .push_str(&format!("\n初期スキャン: {init_ms} ms"));
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
                                    ui.label(
                                        egui::RichText::new(scan_text)
                                            .size(11.0)
                                            .color(scan_color),
                                    )
                                    .on_hover_text(hover);
                                    // 取込
                                    let ingest_text = if v.stats.ingested_failed > 0 {
                                        format!(
                                            "{} (失敗 {})",
                                            v.stats.ingested_ok, v.stats.ingested_failed
                                        )
                                    } else {
                                        format!("{}", v.stats.ingested_ok)
                                    };
                                    let ingest_color = if v.stats.ingested_failed > 0 {
                                        egui::Color32::from_rgb(200, 120, 60)
                                    } else {
                                        egui::Color32::from_gray(160)
                                    };
                                    ui.label(
                                        egui::RichText::new(ingest_text)
                                            .size(11.0)
                                            .color(ingest_color),
                                    );
                                    // 削除
                                    ui.label(
                                        egui::RichText::new(format!("{}", v.stats.deleted))
                                            .size(11.0)
                                            .color(egui::Color32::from_gray(160)),
                                    );
                                    // 操作ボタン
                                    if ui
                                        .button("🔄 再構築")
                                        .on_hover_text(
                                            "このお気に入り配下を再走査して、差分を取り込み直す",
                                        )
                                        .clicked()
                                    {
                                        request_rescan_for = Some(v.favorite_id);
                                    }
                                    ui.end_row();
                                }
                            });
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("🔄 すべて再構築")
                        .on_hover_text("表示中の全お気に入りを再走査")
                        .clicked()
                    {
                        request_rescan_all = true;
                    }
                    if ui.button("閉じる").clicked() {
                        close_requested = true;
                    }
                });

                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(
                        "※ 自動インデックスのフラグ ON/OFF は「お気に入りの編集」から。\n\
                         ※ フォルダ名索引 (Ctrl+S) は旧「インデックス作成」ダイアログを継続利用。",
                    )
                    .weak()
                    .size(10.0),
                );
            });

        if escape_pressed || close_requested || !open {
            self.show_index_manager = false;
        }

        if let Some(mgr) = self.indexer_manager.as_ref() {
            if let Some(id) = request_rescan_for {
                mgr.request_full_rescan(id);
            }
            if request_rescan_all {
                for v in &stats_list {
                    mgr.request_full_rescan(v.favorite_id);
                }
            }
        }
    }
}
