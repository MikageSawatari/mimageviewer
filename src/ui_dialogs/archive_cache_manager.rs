//! 変換済みアーカイブキャッシュ (7z/LZH → ZIP) の管理ダイアログ (v0.7.0)。
//!
//! サムネイルキャッシュとは別のメニュー項目として提供する。
//! キャッシュ 1 エントリは数百 MB 〜 GB になりうるため、
//! ユーザーが一覧から容量を把握して手動で整理できる UI を重視する。
//!
//! - 一覧: 元ファイル名 (存在しないものは ✗ + 赤字)・形式 (7z / LZH)・
//!   キャッシュ ZIP サイズ・画像数
//! - 操作: 個別選択削除 / 元ファイル消失を一括削除 / 全削除 / 再読込

#![allow(unused_imports)]

use std::path::PathBuf;

use eframe::egui;

use crate::app::App;
use crate::ui_helpers::{format_bytes, truncate_name};

impl App {
    /// 変換済みアーカイブキャッシュ管理ダイアログを開くためのフラグ初期化。
    /// メニューから呼ぶこと。ロードはワーカーに回し、ダイアログは空の状態で開く。
    pub(crate) fn open_archive_cache_manager(&mut self) {
        self.archive_cache_manager_result = None;
        self.show_archive_cache_manager = true;
        self.reload_archive_cache_rows();
    }

    pub(crate) fn show_archive_cache_manager_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_archive_cache_manager {
            return;
        }

        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

        egui::Window::new("変換済みアーカイブキャッシュ管理")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                draw_body(self, ui);
            });

        if !open || (escape_pressed && !self.archive_cache_confirm_delete_all) {
            self.show_archive_cache_manager = false;
            self.archive_cache_confirm_delete_all = false;
        }

        self.show_archive_cache_confirm_dialog(ctx);
    }

    fn show_archive_cache_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.archive_cache_confirm_delete_all {
            return;
        }
        let mut confirm_open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        egui::Window::new("アーカイブキャッシュの全削除")
            .open(&mut confirm_open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("すべての変換済みアーカイブキャッシュを削除します。");
                ui.label("元ファイルはそのまま残りますが、再変換には時間がかかります。");
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let busy = self.archive_cache_maint_pending.is_some();
                    let del_btn = egui::Button::new("  削除する  ");
                    if ui.add_enabled(!busy, del_btn).clicked() {
                        if let Some(db) = self.archive_cache_db.clone() {
                            self.archive_cache_maint_pending =
                                Some(crate::cache_maintenance::spawn_archive(
                                    crate::cache_maintenance::ArchiveMaintTask::DeleteAll,
                                    db,
                                ));
                        }
                        self.archive_cache_confirm_delete_all = false;
                    }
                    if ui.button("  キャンセル  ").clicked() || escape_pressed {
                        self.archive_cache_confirm_delete_all = false;
                    }
                });
            });
        if !confirm_open {
            self.archive_cache_confirm_delete_all = false;
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// 本体描画
// ──────────────────────────────────────────────────────────────────────

fn draw_body(app: &mut App, ui: &mut egui::Ui) {
    ui.set_min_width(600.0);

    let Some(db) = app.archive_cache_db.clone() else {
        ui.label(
            egui::RichText::new("キャッシュ DB が初期化できていません。")
                .color(egui::Color32::from_rgb(180, 60, 60)),
        );
        return;
    };

    let busy = app.archive_cache_maint_pending.is_some();
    if busy {
        // worker 完了まで毎フレーム再描画して結果反映を受け取る。
        ui.ctx().request_repaint();
    }
    let row_count = app
        .archive_cache_rows
        .as_ref()
        .map(|v| v.len())
        .unwrap_or(0);
    let missing_count = app
        .archive_cache_rows
        .as_ref()
        .map(|v| v.iter().filter(|e| !e.src_exists).count())
        .unwrap_or(0);
    let total_bytes = app.archive_cache_total_bytes;

    ui.horizontal(|ui| {
        if app.archive_cache_rows.is_none() {
            ui.label("読み込み中…");
        } else {
            ui.label(format!(
                "{} 件 / 合計 {}",
                row_count,
                format_bytes(total_bytes)
            ));
            if missing_count > 0 {
                ui.label(
                    egui::RichText::new(format!("(元ファイル消失: {})", missing_count))
                        .color(egui::Color32::from_rgb(180, 60, 60)),
                );
            }
        }
    });

    ui.add_space(6.0);

    let selected_count = app.archive_cache_selection.len();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !busy && selected_count > 0,
                egui::Button::new(format!("選択を削除 ({})", selected_count)),
            )
            .clicked()
        {
            spawn_delete_selected(app, db.clone());
        }
        if ui
            .add_enabled(
                !busy && missing_count > 0,
                egui::Button::new(format!("元ファイル消失を削除 ({})", missing_count)),
            )
            .clicked()
        {
            app.archive_cache_maint_pending = Some(crate::cache_maintenance::spawn_archive(
                crate::cache_maintenance::ArchiveMaintTask::DeleteMissing,
                db.clone(),
            ));
        }
        if ui
            .add_enabled(!busy && row_count > 0, egui::Button::new("すべて削除"))
            .clicked()
        {
            app.archive_cache_confirm_delete_all = true;
        }
        if ui
            .add_enabled(!busy, egui::Button::new("再読込"))
            .clicked()
        {
            app.reload_archive_cache_rows();
        }
    });

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);

    if app.archive_cache_rows.is_none() {
        // 初回ロード中は placeholder。
    } else if row_count == 0 {
        ui.label(
            egui::RichText::new("変換済みのアーカイブはありません。")
                .italics()
                .color(egui::Color32::from_gray(140)),
        );
    } else {
        draw_entry_list(app, ui);
    }

    if busy {
        ui.add_space(8.0);
        ui.label("処理中…");
    } else if let Some(ref msg) = app.archive_cache_manager_result {
        ui.add_space(8.0);
        ui.label(msg.as_str());
    }
}

fn draw_entry_list(app: &mut App, ui: &mut egui::Ui) {
    let rows = app.archive_cache_rows.clone().unwrap_or_default();

    egui::ScrollArea::vertical()
        .max_height(360.0)
        .id_salt("archive_cache_entries")
        .show(ui, |ui| {
            egui::Grid::new("archive_cache_grid")
                .num_columns(5)
                .striped(true)
                .spacing(egui::vec2(8.0, 3.0))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("").strong());
                    ui.label(egui::RichText::new("元ファイル").strong());
                    ui.label(egui::RichText::new("形式").strong());
                    ui.label(egui::RichText::new("キャッシュサイズ").strong());
                    ui.label(egui::RichText::new("画像数").strong());
                    ui.end_row();

                    for (idx, entry) in rows.iter().enumerate() {
                        let mut selected = app.archive_cache_selection.contains(&idx);
                        if ui.checkbox(&mut selected, "").changed() {
                            if selected {
                                app.archive_cache_selection.insert(idx);
                            } else {
                                app.archive_cache_selection.remove(&idx);
                            }
                        }
                        let name = entry
                            .src_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string();
                        let path_text = entry.src_path.to_string_lossy().to_string();
                        let label = if entry.src_exists {
                            egui::RichText::new(truncate_name(&name, 42))
                        } else {
                            egui::RichText::new(format!("✗ {}", truncate_name(&name, 40)))
                                .color(egui::Color32::from_rgb(180, 60, 60))
                        };
                        ui.label(label).on_hover_text(path_text);
                        ui.label(entry.format.label());
                        ui.label(format_bytes(entry.cached_zip_size.max(0) as u64));
                        ui.label(format!("{}", entry.image_count));
                        ui.end_row();
                    }
                });
        });
}

fn spawn_delete_selected(
    app: &mut App,
    db: std::sync::Arc<crate::archive_cache::ArchiveCacheDb>,
) {
    let Some(rows) = app.archive_cache_rows.as_ref() else {
        return;
    };
    let src_paths: Vec<PathBuf> = app
        .archive_cache_selection
        .iter()
        .filter_map(|idx| rows.get(*idx).map(|e| e.src_path.clone()))
        .collect();
    if src_paths.is_empty() {
        return;
    }
    app.archive_cache_maint_pending = Some(crate::cache_maintenance::spawn_archive(
        crate::cache_maintenance::ArchiveMaintTask::DeleteSelected { src_paths },
        db,
    ));
}
