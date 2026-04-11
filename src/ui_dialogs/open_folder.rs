//! `show_open_folder_dialog_window` ダイアログの実装。
//!
//! アドレスバーを非表示にしている場合でも、フォルダを直接入力して
//! 開けるようにするためのモーダルダイアログ。「ファイル → フォルダを開く…」
//! メニューから呼ばれる。
//!
//! `update()` から `self.show_open_folder_dialog_window(ctx)` で呼ばれ、
//! 決定されたパスを返す (呼び出し側で `load_folder` へ渡す)。

#![allow(unused_imports)]

use std::path::PathBuf;

use eframe::egui;

use crate::app::App;

impl App {
    pub(crate) fn show_open_folder_dialog_window(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<PathBuf> {
        if !self.show_open_folder_dialog {
            return None;
        }

        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let dialog_pos = ctx.content_rect().min + egui::vec2(80.0, 60.0);

        egui::Window::new("フォルダを開く")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(520.0);
                ui.label("開きたいフォルダのパスを入力してください:");
                ui.add_space(4.0);

                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.open_folder_input)
                        .desired_width(f32::INFINITY)
                        .hint_text(r"例: C:\Users\you\Pictures"),
                );
                // 初回フォーカス
                if !resp.has_focus() && !ui.memory(|m| m.focused().is_some()) {
                    resp.request_focus();
                }
                // Enter で決定（フォーカス中 or フォーカスを失った瞬間）
                let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
                if enter && (resp.has_focus() || resp.lost_focus()) {
                    apply = true;
                }

                // エラーメッセージ
                if let Some(ref err) = self.open_folder_error {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(err)
                            .color(egui::Color32::from_rgb(220, 60, 60))
                            .size(13.0),
                    );
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let can_apply = !self.open_folder_input.trim().is_empty();
                    if ui
                        .add_enabled(can_apply, egui::Button::new("  開く  "))
                        .clicked()
                    {
                        apply = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
                // Esc でキャンセル
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });

        let mut result: Option<PathBuf> = None;
        if apply {
            let input = self.open_folder_input.trim().to_string();
            if !input.is_empty() {
                let p = PathBuf::from(&input);
                if p.exists() {
                    if let Some(resolved) = crate::folder_tree::resolve_openable_path(&p) {
                        result = Some(resolved);
                        self.show_open_folder_dialog = false;
                        self.open_folder_input.clear();
                        self.open_folder_error = None;
                    } else {
                        self.open_folder_error =
                            Some(format!("開けるフォルダが見つかりません: {input}"));
                    }
                } else {
                    self.open_folder_error =
                        Some(format!("パスが存在しません: {input}"));
                }
            }
        } else if cancel || !open {
            self.show_open_folder_dialog = false;
            self.open_folder_input.clear();
            self.open_folder_error = None;
        }

        result
    }
}
