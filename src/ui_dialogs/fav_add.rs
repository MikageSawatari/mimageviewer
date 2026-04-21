//! `show_fav_add_dialog_window` ダイアログの実装。
//!
//! お気に入りに現在のフォルダを追加する際に、表示名を入力させるための
//! 小さなモーダルダイアログ。`App` の `show_fav_add_dialog` フラグが
//! true のときだけ描画される。
//!
//! `update()` から `self.show_fav_add_dialog_window(ctx)` で呼ばれる。

#![allow(unused_imports)]

use std::path::PathBuf;

use eframe::egui;

use crate::app::App;

impl App {
    pub(crate) fn show_fav_add_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_fav_add_dialog {
            return;
        }

        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let dialog_pos = ctx.content_rect().min + egui::vec2(80.0, 60.0);
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);

        egui::Window::new("お気に入りに追加")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);

                if let Some(ref target) = self.fav_add_target {
                    ui.label("次のフォルダをお気に入りに追加します:");
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(target.to_string_lossy())
                            .monospace()
                            .weak(),
                    );
                    ui.add_space(8.0);
                    ui.label("表示名 (ツールバーやメニューに表示される名前):");
                    ui.add_space(2.0);

                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.fav_add_name_input)
                            .desired_width(f32::INFINITY),
                    );
                    // 初回フォーカス
                    if !resp.has_focus() && ctx.input(|i| i.focused)
                        && !ui.memory(|m| m.focused().is_some())
                    {
                        resp.request_focus();
                    }
                    // Enter で決定 (IME 変換中は dialog_enter_pressed が false を返す)
                    if resp.lost_focus() && enter_pressed {
                        apply = true;
                    }

                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("自動インデックス (あとで編集可能):")
                            .weak(),
                    );
                    ui.add_space(2.0);
                    ui.checkbox(
                        &mut self.fav_add_auto_index_structure,
                        "フォルダ名 / ZIP・PDF 名で検索できるようにする (Ctrl+S)",
                    );
                    ui.checkbox(
                        &mut self.fav_add_auto_index_metadata,
                        "メタデータ (EXIF / XMP / PNG tEXt) を検索できるようにする (Ctrl+F/G)",
                    );
                    ui.checkbox(
                        &mut self.fav_add_auto_index_thumbs,
                        "サムネイルを事前キャッシュする",
                    );
                } else {
                    ui.label("追加対象のフォルダが不明です。");
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let can_apply = self.fav_add_target.is_some()
                        && !self.fav_add_name_input.trim().is_empty();
                    if ui
                        .add_enabled(can_apply, egui::Button::new("  追加  "))
                        .clicked()
                    {
                        apply = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });

        if apply {
            if let Some(target) = self.fav_add_target.take() {
                let name = self.fav_add_name_input.trim().to_string();
                let added = self.settings.add_favorite(name, target);
                if added {
                    // 追加した最後のエントリに自動インデックスフラグを反映
                    if let Some(last) = self.settings.favorites.last_mut() {
                        last.auto_index_structure = self.fav_add_auto_index_structure;
                        last.auto_index_metadata = self.fav_add_auto_index_metadata;
                        last.auto_index_thumbs = self.fav_add_auto_index_thumbs;
                    }
                    self.settings.save();
                }
                // cache_creator_checked は favorites と同じ長さを保つ
                self.cc.checked
                    .resize(self.settings.favorites.len(), false);
            }
            self.show_fav_add_dialog = false;
            self.fav_add_name_input.clear();
            self.fav_add_target = None;
            // 次回デフォルト値は false リセット (design doc §8.2 の "環境設定のデフォルトから取る" は v1.x で検討)
            self.fav_add_auto_index_structure = false;
            self.fav_add_auto_index_metadata = false;
            self.fav_add_auto_index_thumbs = false;
        } else if cancel || !open || escape_pressed {
            self.show_fav_add_dialog = false;
            self.fav_add_name_input.clear();
            self.fav_add_target = None;
            self.fav_add_auto_index_structure = false;
            self.fav_add_auto_index_metadata = false;
            self.fav_add_auto_index_thumbs = false;
        }
    }
}
