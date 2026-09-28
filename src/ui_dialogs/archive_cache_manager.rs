//! 変換済みアーカイブ (RAR/7z/LZH → ZIP) と EPUB の独立した管理ダイアログ。
//!
//! サムネイルキャッシュとも互いとも別のメニュー項目として提供する。
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
        self.close_epub_cache_manager();
        self.archive_cache_manager_result = None;
        self.show_archive_cache_manager = true;
        if self.archive_cache_maint_pending.is_none() {
            self.reload_archive_cache_rows();
        }
    }

    pub(crate) fn open_epub_cache_manager(&mut self) {
        self.close_archive_cache_manager();
        self.epub_cache_manager_result = None;
        self.show_epub_cache_manager = true;
        if self.epub_cache_maint_pending.is_none() {
            self.epub_cache_rows = None;
            self.epub_cache_maint_pending = Some(crate::cache_maintenance::spawn_epub(
                crate::cache_maintenance::EpubMaintTask::LoadRows,
                crate::data_dir::get(),
            ));
        }
    }

    pub(crate) fn show_archive_cache_manager_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_archive_cache_manager {
            return;
        }

        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let (safe_rect, dialog_size) = manager_geometry(ctx);

        egui::Window::new("変換済みアーカイブ管理")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
            .default_size(dialog_size)
            .max_size(safe_rect.size())
            .constrain_to(safe_rect)
            .show(ctx, |ui| {
                draw_archive_body(self, ui);
            });

        if !open || (escape_pressed && !self.archive_cache_confirm_delete_all) {
            self.close_archive_cache_manager();
        }

        self.show_archive_cache_confirm_dialog(ctx);
    }

    fn close_archive_cache_manager(&mut self) {
        close_archive_cache_manager_flags(
            &mut self.show_archive_cache_manager,
            &mut self.archive_cache_confirm_delete_all,
        );
    }

    pub(crate) fn show_epub_cache_manager_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_epub_cache_manager {
            return;
        }
        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let (safe_rect, dialog_size) = manager_geometry(ctx);
        egui::Window::new("EPUB 変換キャッシュ管理")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
            .default_size(dialog_size)
            .max_size(safe_rect.size())
            .constrain_to(safe_rect)
            .show(ctx, |ui| draw_epub_body(self, ui));
        if epub_manager_should_close(
            open,
            escape_pressed,
            self.epub_cache_confirm_delete_all,
            self.epub_cache_maint_pending
                .as_ref()
                .is_some_and(|pending| {
                    !matches!(
                        pending.task,
                        crate::cache_maintenance::EpubMaintTask::LoadRows
                    )
                }),
        ) {
            self.close_epub_cache_manager();
        }
        self.show_epub_cache_confirm_dialog(ctx);
    }

    fn close_epub_cache_manager(&mut self) {
        close_epub_cache_manager_flags(
            &mut self.show_epub_cache_manager,
            &mut self.epub_cache_confirm_delete_all,
        );
    }

    pub(crate) fn poll_epub_cache_maint_pending(&mut self) {
        let Some(pending) = self.epub_cache_maint_pending.as_ref() else {
            return;
        };
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.epub_cache_maint_pending = None;
                self.epub_cache_manager_result = Some("一覧を読み込めませんでした。".into());
                return;
            }
        };
        self.epub_cache_maint_pending = None;
        self.invalidate_removed_epub_generations(&result.physically_removed);
        if let Some(error) = result.error {
            crate::logger::log(format!("epub cache manager: {error}"));
            self.epub_cache_manager_result =
                Some("処理できませんでした。詳細はログを確認してください。".into());
        } else {
            if result.deleted > 0 || !result.failures.is_empty() {
                self.epub_cache_manager_result =
                    Some(format_epub_delete_result(result.deleted, &result.failures));
            }
            self.epub_cache_selection.retain(|id| {
                result
                    .entries
                    .iter()
                    .any(|entry| entry.generation.generation_id == *id)
            });
            self.epub_cache_rows = Some(result.entries);
        }
    }

    fn show_epub_cache_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.epub_cache_confirm_delete_all {
            return;
        }
        let mut open = true;
        egui::Window::new("EPUB の変換結果をすべて削除")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("すべての EPUB の変換結果をその場で削除します。使用中の本は残ります。");
                ui.label("元の EPUB は残ります。再度読むには変換が必要です。");
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.epub_cache_maint_pending.is_none(),
                            egui::Button::new("削除する"),
                        )
                        .clicked()
                    {
                        self.epub_cache_maint_pending = Some(crate::cache_maintenance::spawn_epub(
                            crate::cache_maintenance::EpubMaintTask::DeleteAll,
                            crate::data_dir::get(),
                        ));
                        self.epub_cache_confirm_delete_all = false;
                    }
                    if ui.button("キャンセル").clicked() {
                        self.epub_cache_confirm_delete_all = false;
                    }
                });
            });
        if !open {
            self.epub_cache_confirm_delete_all = false;
        }
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

fn epub_manager_should_close(open: bool, escape: bool, confirming_all: bool, busy: bool) -> bool {
    !busy && (!open || (escape && !confirming_all))
}

fn format_epub_delete_result(deleted: usize, failures: &[(PathBuf, String)]) -> String {
    let mut message = format!("{deleted} 件を削除しました。");
    for (path, reason) in failures {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("EPUB");
        message.push_str(&format!("\n{name}: {reason}"));
    }
    message
}

fn close_archive_cache_manager_flags(show: &mut bool, archive: &mut bool) {
    *show = false;
    *archive = false;
}

fn close_epub_cache_manager_flags(show: &mut bool, epub: &mut bool) {
    *show = false;
    *epub = false;
}

fn manager_geometry(ctx: &egui::Context) -> (egui::Rect, egui::Vec2) {
    let safe_rect = ctx.content_rect().shrink(16.0);
    let size = egui::vec2(740.0, 600.0).min(safe_rect.size());
    (safe_rect, size)
}

// ──────────────────────────────────────────────────────────────────────
// 本体描画
// ──────────────────────────────────────────────────────────────────────

fn draw_archive_body(app: &mut App, ui: &mut egui::Ui) {
    ui.set_min_width(600.0_f32.min(ui.available_width()));

    let Some(db) = app.archive_cache_db.clone() else {
        ui.label(
            egui::RichText::new("キャッシュ DB が初期化できていません。")
                .color(ui.visuals().error_fg_color),
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
                        .color(ui.visuals().error_fg_color),
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

    if busy {
        ui.label("処理中…");
    } else if let Some(ref msg) = app.archive_cache_manager_result {
        ui.label(msg.as_str());
    }

    if app.archive_cache_rows.is_none() {
        ui.label("読み込み中…");
    } else if row_count == 0 {
        ui.label(
            egui::RichText::new("変換済みのアーカイブはありません。")
                .italics()
                .color(ui.visuals().weak_text_color()),
        );
    } else {
        draw_entry_list(app, ui);
    }
}

fn draw_epub_body(app: &mut App, ui: &mut egui::Ui) {
    ui.set_min_width(600.0_f32.min(ui.available_width()));
    ui.label("変換結果はその場で削除します。表示中など使用中の本は削除できず、一覧に残ります。元の EPUB は残ります。");
    let busy = app.epub_cache_maint_pending.is_some();
    if busy {
        ui.ctx().request_repaint();
    }
    let rows = app.epub_cache_rows.clone();
    let active_count = rows.as_ref().map_or(0, Vec::len);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !busy && !app.epub_cache_selection.is_empty(),
                egui::Button::new("選択を削除"),
            )
            .clicked()
        {
            app.epub_cache_maint_pending = Some(crate::cache_maintenance::spawn_epub(
                crate::cache_maintenance::EpubMaintTask::DeleteSelected {
                    generation_ids: app.epub_cache_selection.iter().copied().collect(),
                },
                crate::data_dir::get(),
            ));
        }
        if ui
            .add_enabled(!busy && active_count > 0, egui::Button::new("すべて削除"))
            .clicked()
        {
            app.epub_cache_confirm_delete_all = true;
        }
        if ui
            .add_enabled(
                !busy && active_count > 0,
                egui::Button::new("元ファイル消失を削除"),
            )
            .clicked()
        {
            app.epub_cache_maint_pending = Some(crate::cache_maintenance::spawn_epub(
                crate::cache_maintenance::EpubMaintTask::DeleteMissingSources,
                crate::data_dir::get(),
            ));
        }
        if ui.add_enabled(!busy, egui::Button::new("再読込")).clicked() {
            app.epub_cache_rows = None;
            app.epub_cache_maint_pending = Some(crate::cache_maintenance::spawn_epub(
                crate::cache_maintenance::EpubMaintTask::LoadRows,
                crate::data_dir::get(),
            ));
        }
    });
    if let Some(result) = &app.epub_cache_manager_result {
        egui::ScrollArea::vertical()
            .max_height(120.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for line in result.lines() {
                    ui.label(line);
                }
            });
    }
    if let Some(rows) = rows {
        if rows.is_empty() {
            ui.label("変換済みの EPUB はありません。");
        } else {
            epub_cache_entry_scroll_area(ui.available_height()).show(ui, |ui| {
                egui::Grid::new("epub_cache_grid")
                    .num_columns(6)
                    .striped(true)
                    .show(ui, |ui| {
                        for heading in [
                            "",
                            "元ファイル",
                            "ページ数",
                            "保存サイズ",
                            "最終利用",
                            "状態",
                        ] {
                            ui.strong(heading);
                        }
                        ui.end_row();
                        for row in &rows {
                            let id = row.generation.generation_id;
                            let mut selected = app.epub_cache_selection.contains(&id);
                            if ui
                                .add_enabled(!busy, egui::Checkbox::new(&mut selected, ""))
                                .changed()
                            {
                                if selected {
                                    app.epub_cache_selection.insert(id);
                                } else {
                                    app.epub_cache_selection.remove(&id);
                                }
                            }
                            let name = row
                                .generation
                                .src_path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("?");
                            ui.label(truncate_name(name, 40))
                                .on_hover_text(row.generation.src_path.display().to_string());
                            ui.label(row.generation.page_count.to_string());
                            ui.label(format_bytes(row.generation.pdf_size));
                            ui.label(crate::app::format_details_timestamp(
                                row.last_access_at,
                                false,
                            ));
                            ui.label(if row.retired {
                                "旧変換結果"
                            } else {
                                "利用可能"
                            });
                            ui.end_row();
                        }
                    });
            });
        }
    } else {
        ui.label("読み込み中…");
    }
}

fn draw_entry_list(app: &mut App, ui: &mut egui::Ui) {
    let rows = app.archive_cache_rows.clone().unwrap_or_default();

    archive_cache_entry_scroll_area(ui.available_height()).show(ui, |ui| {
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
                            .color(ui.visuals().error_fg_color)
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

fn archive_cache_entry_scroll_area(height: f32) -> egui::ScrollArea {
    egui::ScrollArea::vertical()
        .max_height(height.max(1.0))
        .id_salt("archive_cache_entries")
        // 横方向を内容幅へ縮めない。ダイアログの利用可能幅を使い切ることで、縦
        // スクロールバーを表の途中ではなくダイアログ右端へ固定する。
        .auto_shrink([false, false])
}

fn epub_cache_entry_scroll_area(height: f32) -> egui::ScrollArea {
    egui::ScrollArea::vertical()
        .max_height(height.max(1.0))
        .id_salt("epub_cache_entries")
        .auto_shrink([false, false])
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
    fn epub_delete_result_reports_partial_success_and_each_failure() {
        let message = format_epub_delete_result(
            2,
            &[
                (
                    PathBuf::from("C:/books/open.epub"),
                    "表示中のため削除できませんでした".into(),
                ),
                (
                    PathBuf::from("C:/books/busy.epub"),
                    "使用中のため削除できませんでした".into(),
                ),
            ],
        );
        assert!(message.starts_with("2 件を削除しました。"));
        assert!(message.contains("open.epub: 表示中のため"));
        assert!(message.contains("busy.epub: 使用中のため"));
    }

    #[test]
    fn epub_manager_stays_modal_until_background_delete_result_arrives() {
        assert!(!epub_manager_should_close(false, false, false, true));
        assert!(!epub_manager_should_close(true, true, false, true));
        assert!(epub_manager_should_close(false, false, false, false));
    }

    #[test]
    fn closing_each_manager_clears_only_its_own_confirmation() {
        let (mut archive_show, mut archive_confirm) = (true, true);
        let (mut epub_show, mut epub_confirm) = (true, true);
        close_archive_cache_manager_flags(&mut archive_show, &mut archive_confirm);
        assert!(!archive_show && !archive_confirm);
        assert!(epub_show && epub_confirm);
        close_epub_cache_manager_flags(&mut epub_show, &mut epub_confirm);
        assert!(!epub_show && !epub_confirm);
    }

    #[test]
    fn window_close_branch_clears_epub_delete_all_confirmation() {
        let mut env = crate::app::tests::phase_c_support::setup_app();
        let app = &mut *env;
        app.show_epub_cache_manager = true;
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts_with_settings(&ctx, &app.settings.ui_font);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            ..Default::default()
        };
        for _ in 0..3 {
            let _ = ctx.run(input(), |ctx| app.show_epub_cache_manager_dialog(ctx));
        }
        let rect = ctx
            .memory(|memory| memory.area_rect(egui::Id::new("EPUB 変換キャッシュ管理")))
            .expect("manager window must be laid out");
        let style = ctx.style();
        let frame = egui::Frame::window(&style);
        let heading = egui::TextStyle::Heading.resolve(&style);
        let title_inner_height = ctx.fonts_mut(|fonts| fonts.row_height(&heading));
        let title_height =
            title_inner_height.max(style.spacing.interact_size.y) + frame.inner_margin.sum().y;
        let close = egui::pos2(
            rect.right() - frame.stroke.width - title_height * 0.5,
            rect.top() + frame.stroke.width + title_height * 0.5,
        );
        app.epub_cache_confirm_delete_all = true;
        for pressed in [true, false] {
            let mut frame = input();
            frame.events.push(egui::Event::PointerMoved(close));
            frame.events.push(egui::Event::PointerButton {
                pos: close,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            let _ = ctx.run(frame, |ctx| app.show_epub_cache_manager_dialog(ctx));
        }
        assert!(!app.show_epub_cache_manager);
        assert!(!app.epub_cache_confirm_delete_all);
    }

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
                let output =
                    archive_cache_entry_scroll_area(ui.available_height()).show(ui, |ui| {
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

    #[test]
    fn manager_table_fills_available_height() {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 700.0),
            )),
            ..Default::default()
        };
        let mut heights = (0.0, 0.0);
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_height(500.0);
                heights.0 = archive_cache_entry_scroll_area(ui.available_height())
                    .show(ui, |ui| {
                        ui.label("row");
                    })
                    .inner_rect
                    .height();
                heights.1 = ui.available_height();
            });
        });
        assert!(heights.0 > 400.0, "table height: {}", heights.0);
        assert!(heights.1 < 100.0, "unused height: {}", heights.1);
    }

    #[test]
    fn epub_table_fills_available_height() {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 700.0),
            )),
            ..Default::default()
        };
        let mut height = 0.0;
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_height(500.0);
                height = epub_cache_entry_scroll_area(ui.available_height())
                    .show(ui, |ui| {
                        ui.label("row");
                    })
                    .inner_rect
                    .height();
            });
        });
        assert!(height > 400.0, "EPUB table height: {height}");
    }
}
