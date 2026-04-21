//! `show_favorites_editor_dialog` ダイアログの実装。
//!
//! `App` への impl 拡張として書かれており、フィールドアクセスは
//! `pub(crate)` 経由で行われる。`update()` から `self.show_favorites_editor_dialog(ctx)` で呼ばれる。

#![allow(unused_imports)]

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};

use eframe::egui;

use crate::app::App;
use crate::catalog;
use crate::folder_tree;
use crate::gpu_info;
use crate::grid_item::{GridItem, ThumbnailState};
use crate::settings;
use crate::stats;
use crate::thumb_loader::{
    build_and_save_one, compute_display_px, CacheDecision, LoadRequest, ThumbMsg,
};
use crate::ui_helpers::{
    draw_format_rows, draw_histogram, format_bytes, format_bytes_small, format_count,
    natural_sort_key, truncate_name,
};

impl App {
    pub(crate) fn show_favorites_editor_dialog(&mut self, ctx: &egui::Context) {
        // ── お気に入り編集ポップアップ ───────────────────────────────
        if !self.show_favorites_editor {
            return;
        }
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let mut swap: Option<(usize, usize)> = None;
        let mut remove: Option<usize> = None;
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        let escape_pressed = self.dialog_escape_pressed(ctx);

        let scroll_max_h = (ctx.content_rect().height() - 180.0).min(640.0).max(80.0);
        egui::Window::new("お気に入りの編集")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(1040.0);
                if self.settings.favorites.is_empty() {
                    ui.label("お気に入りはまだ登録されていません。");
                } else {
                    ui.label(
                        egui::RichText::new(
                            "表示名を編集できます。右側のチェックで自動インデックスの対象を選択できます。",
                        )
                        .weak(),
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
                            // ヘッダ
                            ui.label(egui::RichText::new("表示名").strong());
                            ui.label(egui::RichText::new("パス").strong());
                            ui.label(egui::RichText::new("構造").strong())
                                .on_hover_text(
                                    "Ctrl+S フォルダ/ZIP/PDF 名検索のインデックス対象にする\n\
                                     (自動で増減を追跡)",
                                );
                            ui.label(egui::RichText::new("メタ").strong())
                                .on_hover_text(
                                    "Ctrl+F/G 全文メタデータ検索のインデックス対象にする\n\
                                     (EXIF / XMP / PNG tEXt を含む)",
                                );
                            ui.label(egui::RichText::new("サムネ").strong())
                                .on_hover_text("サムネイルを事前キャッシュする");
                            ui.label(egui::RichText::new("操作").strong());
                            ui.end_row();

                            for i in 0..n {
                                // 表示名 (編集可能) — Grid 内では desired_width が
                                // 効かないため add_sized で強制指定する
                                let name_resp = ui.add_sized(
                                    [100.0, 20.0],
                                    egui::TextEdit::singleline(
                                        &mut self.settings.favorites[i].name,
                                    ),
                                );
                                let _ = name_resp;

                                // パス (読み取り専用表示)
                                let path_str = self.settings.favorites[i]
                                    .path
                                    .to_string_lossy()
                                    .to_string();
                                ui.label(
                                    egui::RichText::new(truncate_name(&path_str, 56))
                                        .monospace()
                                        .weak(),
                                )
                                .on_hover_text(&path_str);

                                // 自動インデックス フラグ 3 つ (v0.8.0 新規)
                                ui.checkbox(
                                    &mut self.settings.favorites[i].auto_index_structure,
                                    "",
                                );
                                ui.checkbox(
                                    &mut self.settings.favorites[i].auto_index_metadata,
                                    "",
                                );
                                ui.checkbox(
                                    &mut self.settings.favorites[i].auto_index_thumbs,
                                    "",
                                );

                                // 操作ボタン
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
                    // 一括 ON/OFF ボタン (design doc §8.1 mockup)
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("一括操作:").weak());
                        if ui.button("構造 全ON").clicked() {
                            for f in &mut self.settings.favorites { f.auto_index_structure = true; }
                        }
                        if ui.button("構造 全OFF").clicked() {
                            for f in &mut self.settings.favorites { f.auto_index_structure = false; }
                        }
                        if ui.button("メタ 全ON").clicked() {
                            for f in &mut self.settings.favorites { f.auto_index_metadata = true; }
                        }
                        if ui.button("メタ 全OFF").clicked() {
                            for f in &mut self.settings.favorites { f.auto_index_metadata = false; }
                        }
                        if ui.button("サムネ 全ON").clicked() {
                            for f in &mut self.settings.favorites { f.auto_index_thumbs = true; }
                        }
                        if ui.button("サムネ 全OFF").clicked() {
                            for f in &mut self.settings.favorites { f.auto_index_thumbs = false; }
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

        if apply {
            self.settings.save();
            // インデクサに反映 (v0.8.0): ON/OFF 切り替えで Supervisor を spawn/stop する。
            // Supervisor drop は thread join を伴うが、OK 押下は「ユーザが待ってもよい」
            // タイミングなので UI ブロックは許容。
            if let Some(mgr) = self.indexer_manager.as_mut() {
                mgr.sync_with_favorites(&self.settings.favorites);
            }
            self.show_favorites_editor = false;
        } else if cancel || !open {
            // キャンセル: 設定を再読み込みして変更を破棄
            self.settings = crate::settings::Settings::load();
            self.show_favorites_editor = false;
        }
    }
}
