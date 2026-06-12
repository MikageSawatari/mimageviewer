//! タグ編集ダイアログ (docs/tag-feature.md §4.1)。
//!
//! ユーザがタグ一覧を編集するための UI。お気に入り編集 (`favorites_editor.rs`) と
//! 同じ構造で、表示名編集 / 並べ替え / 削除 / 末尾への新規追加 をサポート。
//! よく使うタグをメニュー / ツールバーへピン留めするための管理 UI。

use eframe::egui;

use crate::app::App;
use crate::settings::TagDef;

impl App {
    /// ダイアログを開く (呼び出し側で `show_tag_editor = true` にする前に呼ぶ)。
    /// 現在の Settings のタグ一覧を draft にコピーする。
    pub(crate) fn open_tag_editor(&mut self) {
        self.tag_editor_draft = self.settings.tags.clone();
        self.show_tag_editor = true;
    }

    pub(crate) fn show_tag_editor_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_tag_editor {
            return;
        }
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);

        let mut swap: Option<(usize, usize)> = None;
        let mut remove: Option<usize> = None;
        let mut add_empty_row = false;

        egui::Window::new("タグの管理")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(480.0);

                ui.label(
                    egui::RichText::new(
                        "よく使うタグをピン留めすると、メニューとツールバーに表示されます。",
                    )
                    .size(11.0)
                    .weak(),
                );
                ui.add_space(6.0);

                let n = self.tag_editor_draft.len();
                egui::ScrollArea::vertical()
                    .id_salt("tag_edit_scroll")
                    .max_height(360.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Grid::new("tag_edit_grid")
                            .striped(true)
                            .num_columns(4)
                            .spacing([8.0, 4.0])
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new("タグ名").strong());
                                ui.label(egui::RichText::new("プレビュー").strong());
                                ui.label(egui::RichText::new("表示").strong());
                                ui.label(egui::RichText::new("操作").strong());
                                ui.end_row();

                                for i in 0..n {
                                    let resp = ui.add_sized(
                                        [200.0, 20.0],
                                        egui::TextEdit::singleline(
                                            &mut self.tag_editor_draft[i].name,
                                        ),
                                    );
                                    // Enter で次行 or 追加行作成 (未実装、v1.1 検討)
                                    let _ = resp;

                                    // プレビュー (#付き、空なら "—")
                                    let name = self.tag_editor_draft[i].name.trim();
                                    let preview = if name.is_empty() {
                                        "—".to_string()
                                    } else {
                                        format!("#{name}")
                                    };
                                    ui.label(
                                        egui::RichText::new(preview)
                                            .monospace()
                                            .color(egui::Color32::from_rgb(100, 170, 100)),
                                    );

                                    ui.checkbox(
                                        &mut self.tag_editor_draft[i].show_shortcut,
                                        "ピン留め",
                                    );

                                    // 操作
                                    ui.horizontal(|ui| {
                                        let up_en = i > 0;
                                        let dn_en = i + 1 < n;
                                        if ui.add_enabled(up_en, egui::Button::new("↑")).clicked()
                                        {
                                            swap = Some((i - 1, i));
                                        }
                                        if ui.add_enabled(dn_en, egui::Button::new("↓")).clicked()
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

                ui.add_space(6.0);
                if ui.button("＋ タグを追加").clicked() {
                    add_empty_row = true;
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        apply = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });

                if enter_pressed {
                    apply = true;
                }
                if escape_pressed {
                    cancel = true;
                }
            });

        if let Some((a, b)) = swap {
            self.tag_editor_draft.swap(a, b);
        }
        if let Some(i) = remove {
            self.tag_editor_draft.remove(i);
        }
        if add_empty_row {
            self.tag_editor_draft.push(TagDef::new(String::new()));
        }

        if apply {
            // バリデーション:
            // - 空行は破棄
            // - 先頭 `#` は剥がす (ユーザが `#` を付けて入力した救済)
            // - 重複は先着優先 (tag_key でキー判定)
            let mut seen = std::collections::HashSet::new();
            let mut cleaned: Vec<TagDef> = Vec::new();
            for t in self.tag_editor_draft.drain(..) {
                let mut name = t.name.trim().to_string();
                while name.starts_with('#') {
                    name.remove(0);
                }
                let name = crate::tags_db::normalize_tag_display_name(&name);
                if name.is_empty() {
                    continue;
                }
                if name.chars().count() > 64 {
                    continue;
                }
                let tag_key = crate::tags_db::normalize_tag_key(&name);
                if tag_key.is_empty() || !seen.insert(tag_key.clone()) {
                    continue;
                }
                cleaned.push(TagDef {
                    id: t.id,
                    tag_key,
                    name,
                    show_shortcut: t.show_shortcut,
                });
            }
            self.settings.tags = cleaned;
            self.settings.save();
            self.show_tag_editor = false;
            self.tag_editor_draft.clear();
        } else if cancel || !open {
            self.show_tag_editor = false;
            self.tag_editor_draft.clear();
        }
    }
}
