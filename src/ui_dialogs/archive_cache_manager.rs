//! 変換済みアーカイブキャッシュ (RAR/7z/LZH → ZIP) の管理ダイアログ。
//!
//! サムネイルキャッシュとは別のメニュー項目として提供する。
//! キャッシュ 1 エントリは数百 MB 〜 GB になりうるため、
//! ユーザーが一覧から容量を把握して手動で整理できる UI を重視する。
//!
//! - 一覧: 元ファイル名 (存在しないものは ✗ + 赤字)・形式 (RAR / 7z / LZH / ZIP /
//!   旧形式・不明)・キャッシュ ZIP サイズ・画像数
//! - 操作: 個別選択削除 / 元ファイル消失を一括削除 / 全削除 / 再読込

#![allow(unused_imports)]

use std::path::PathBuf;

use eframe::egui;

use crate::app::App;
use crate::archive_cache::ArchiveCacheEntry;
use crate::ui_helpers::{format_bytes, truncate_name};

impl App {
    /// 変換済みアーカイブキャッシュ管理ダイアログを開くためのフラグ初期化。
    /// メニューから呼ぶこと。ロードはワーカーに回し、ダイアログは空の状態で開く。
    ///
    /// すでに worker 実行中 (例: 削除 pending) の場合は reload を spawn し直さない。
    /// 上書きすると走行中 worker の完了メッセージを受け取れなくなり、削除後の再ロードや
    /// 完了トーストが失われる。worker は完了時に自分で適切な状態 (LoadRows なら rows 更新、
    /// Delete* なら poll 側で reload 再 spawn) に遷移するので、open の責務はダイアログを
    /// 見えるようにするだけでよい。
    pub(crate) fn open_archive_cache_manager(&mut self) {
        self.archive_cache_manager_result = None;
        self.show_archive_cache_manager = true;
        if self.archive_cache_maint_pending.is_none() {
            self.reload_archive_cache_rows();
        }
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
                    // 変換進行中は削除ボタンを無効化 (本体ダイアログと同じ不変条件)
                    let convert_in_flight = self.archive_convert.is_some();
                    let del_btn = egui::Button::new("  削除する  ");
                    if ui
                        .add_enabled(!busy && !convert_in_flight, del_btn)
                        .clicked()
                    {
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
    // 変換進行中は delete 系操作をブロックする。convert_lock は record と maintenance を
    // 排他するが、ConvertDone 送信 ↔ UI 受信 ↔ pending_nav 消費の順序レースまでは閉じないため、
    // UI 層で delete 系の起動自体を止めるのが確実。LoadRows (再読込) は削除しないので許可。
    let convert_in_flight = app.archive_convert.is_some();
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
    let delete_allowed = !busy && !convert_in_flight;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                delete_allowed && selected_count > 0,
                egui::Button::new(format!("選択を削除 ({})", selected_count)),
            )
            .clicked()
        {
            spawn_delete_selected(app, db.clone());
        }
        if ui
            .add_enabled(
                delete_allowed && missing_count > 0,
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
            .add_enabled(
                delete_allowed && row_count > 0,
                egui::Button::new("すべて削除"),
            )
            .clicked()
        {
            app.archive_cache_confirm_delete_all = true;
        }
        if ui.add_enabled(!busy, egui::Button::new("再読込")).clicked() {
            app.reload_archive_cache_rows();
        }
    });
    if convert_in_flight {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("(変換中は削除操作を無効化しています)")
                .small()
                .weak(),
        );
    }

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

    archive_cache_entry_scroll_area().show(ui, |ui| {
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
                    let format_resp = ui.label(format_display_text(entry));
                    if let Some(hover) = format_hover_text(entry) {
                        format_resp.on_hover_text(hover);
                    }
                    ui.label(format_bytes(entry.cached_zip_size.max(0) as u64));
                    ui.label(format!("{}", entry.image_count));
                    ui.end_row();
                }
            });
    });
}

fn archive_cache_entry_scroll_area() -> egui::ScrollArea {
    egui::ScrollArea::vertical()
        .max_height(360.0)
        .id_salt("archive_cache_entries")
        // 横方向を内容幅へ縮めない。ダイアログの利用可能幅を使い切ることで、縦
        // スクロールバーを表の途中ではなくダイアログ右端へ固定する。
        .auto_shrink([false, true])
}

fn format_display_text(entry: &ArchiveCacheEntry) -> String {
    let mut label = match entry.format {
        Some(format) => format.label().to_string(),
        None => {
            let raw = entry.format_raw.trim();
            if raw.is_empty() {
                "旧形式 / 不明".to_string()
            } else {
                format!("旧形式 / 不明 ({})", truncate_name(raw, 16))
            }
        }
    };
    if entry.password_required {
        label.push_str(" / PW");
    }
    label
}

fn format_hover_text(entry: &ArchiveCacheEntry) -> Option<String> {
    let mut lines = Vec::new();
    if entry.format.is_none() {
        let raw = entry.format_raw.trim();
        if raw.is_empty() {
            lines.push("DB の format 値が空です。".to_string());
        } else {
            lines.push(format!("DB の format 値: {raw}"));
        }
    }
    if entry.password_required {
        lines.push(
            "パスワード付き RAR から作成したキャッシュです。ZIP キャッシュ自体は暗号化されていません。"
                .to_string(),
        );
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn spawn_delete_selected(app: &mut App, db: std::sync::Arc<crate::archive_cache::ArchiveCacheDb>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_cache_scroll_area_uses_the_full_dialog_width() {
        let ctx = egui::Context::default();
        let mut inner_width = 0.0;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_width(600.0);
                let output = archive_cache_entry_scroll_area().show(ui, |ui| {
                    ui.set_min_width(120.0);
                    for _ in 0..40 {
                        ui.label("row");
                    }
                });
                inner_width = output.inner_rect.width();
            });
        });

        assert!(
            inner_width > 550.0,
            "scroll body should span the 600px dialog body, got {inner_width}"
        );
    }
}
