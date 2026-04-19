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
    /// メニューから呼ぶこと。
    pub(crate) fn open_archive_cache_manager(&mut self) {
        self.archive_cache_rows = None;
        self.archive_cache_selection.clear();
        self.archive_cache_manager_result = None;
        self.show_archive_cache_manager = true;
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
                    if ui.button("  削除する  ").clicked() {
                        if let Some(db) = self.archive_cache_db.as_ref() {
                            match db.clear_all() {
                                Ok(n) => {
                                    self.archive_cache_manager_result = Some(format!(
                                        "{} 件のキャッシュを削除しました。",
                                        n
                                    ));
                                }
                                Err(e) => {
                                    self.archive_cache_manager_result =
                                        Some(format!("削除失敗: {e}"));
                                }
                            }
                        }
                        self.archive_cache_rows = None;
                        self.archive_cache_selection.clear();
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

fn ensure_rows_loaded(app: &mut App) {
    if app.archive_cache_rows.is_some() {
        return;
    }
    let rows = app
        .archive_cache_db
        .as_ref()
        .and_then(|db| db.list_all().ok())
        .unwrap_or_default();
    app.archive_cache_rows = Some(rows);
    app.archive_cache_selection.clear();
}

fn invalidate_rows(app: &mut App) {
    app.archive_cache_rows = None;
    app.archive_cache_selection.clear();
}

fn draw_body(app: &mut App, ui: &mut egui::Ui) {
    ui.set_min_width(600.0);

    ensure_rows_loaded(app);

    let Some(db) = app.archive_cache_db.clone() else {
        ui.label(
            egui::RichText::new("キャッシュ DB が初期化できていません。")
                .color(egui::Color32::from_rgb(180, 60, 60)),
        );
        return;
    };

    let total_bytes = db.total_size().unwrap_or(0);
    let row_count = app.archive_cache_rows.as_ref().map(|v| v.len()).unwrap_or(0);
    let missing_count = app
        .archive_cache_rows
        .as_ref()
        .map(|v| v.iter().filter(|e| !e.src_exists).count())
        .unwrap_or(0);

    ui.horizontal(|ui| {
        ui.label(format!(
            "{} 件 / 合計 {}",
            row_count,
            format_bytes(total_bytes)
        ));
        if missing_count > 0 {
            ui.label(
                egui::RichText::new(format!("（元ファイル消失: {}）", missing_count))
                    .color(egui::Color32::from_rgb(180, 60, 60)),
            );
        }
    });

    ui.add_space(6.0);

    let selected_count = app.archive_cache_selection.len();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                selected_count > 0,
                egui::Button::new(format!("選択を削除 ({})", selected_count)),
            )
            .clicked()
        {
            delete_selected(app, db.as_ref());
        }
        if ui
            .add_enabled(
                missing_count > 0,
                egui::Button::new(format!("元ファイル消失を削除 ({})", missing_count)),
            )
            .clicked()
        {
            if let Ok(n) = db.delete_missing_originals() {
                app.archive_cache_manager_result = Some(format!(
                    "{} 件のキャッシュ (元ファイル消失) を削除しました。",
                    n
                ));
            }
            invalidate_rows(app);
        }
        if ui
            .add_enabled(row_count > 0, egui::Button::new("すべて削除"))
            .clicked()
        {
            app.archive_cache_confirm_delete_all = true;
        }
        if ui.button("再読込").clicked() {
            invalidate_rows(app);
        }
    });

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);

    if row_count == 0 {
        ui.label(
            egui::RichText::new("変換済みのアーカイブはありません。")
                .italics()
                .color(egui::Color32::from_gray(140)),
        );
    } else {
        draw_entry_list(app, ui);
    }

    if let Some(ref msg) = app.archive_cache_manager_result {
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
                        let mut selected =
                            app.archive_cache_selection.contains(&idx);
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

fn delete_selected(
    app: &mut App,
    db: &crate::archive_cache::ArchiveCacheDb,
) {
    let Some(rows) = app.archive_cache_rows.as_ref() else {
        return;
    };
    let to_delete: Vec<PathBuf> = app
        .archive_cache_selection
        .iter()
        .filter_map(|idx| rows.get(*idx).map(|e| e.src_path.clone()))
        .collect();
    let mut removed = 0;
    for p in &to_delete {
        if db.delete_entry(p).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        app.archive_cache_manager_result =
            Some(format!("{} 件のキャッシュを削除しました。", removed));
    }
    invalidate_rows(app);
}
